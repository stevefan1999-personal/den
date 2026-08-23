//! Compile-time selected WebAssembly engine.
//!
//! Exactly one of the `wasmtime` / `wasmi` features is active at a time, and
//! each backend submodule exports the *same* item names. Everything above this
//! module (`module.rs`, `instance.rs`, `memory.rs`, …) names
//! `crate::backend::*` and is written once.
//!
//! The two engines disagree in a handful of places (constructor arity, error
//! types, whether a store argument is taken, which reference types exist).
//! Those differences are papered over by the shim functions below rather than
//! by a trait: at any given compile there is exactly one implementation, so a
//! trait would buy nothing but generic plumbing.
//!
//! # Which proposals each build accepts
//!
//! Both `new_engine`s spell their proposal set out rather than inheriting the
//! engine's defaults, because the two sets of defaults are *not* the same and
//! a difference there is invisible until a module mysteriously fails to
//! validate on one build only. What is left is the list below; the right-hand
//! column names the constant that reports the difference to Rust and to the
//! parity tests.
//!
//! | Proposal                        | wasmtime | wasmi | reported by |
//! |---------------------------------|----------|-------|-------------|
//! | MVP + mutable-global            | yes      | yes   | —           |
//! | sign-extension, sat-float-to-int| yes      | yes   | —           |
//! | multi-value, multi-memory       | yes      | yes   | —           |
//! | bulk-memory, reference-types    | yes      | yes   | —           |
//! | tail-call, extended-const       | yes      | yes   | —           |
//! | memory64                        | yes      | yes   | —           |
//! | threads (shared memory, atomics)| **off**  | none  | [`SUPPORTS_SHARED_MEMORY`] |
//! | custom-page-sizes               | off      | off   | —           |
//! | wide-arithmetic                 | off      | off   | —           |
//! | simd, relaxed-simd              | yes      | yes   | [`SUPPORTS_V128`] |
//! | gc / `anyref`                   | yes      | no    | [`SUPPORTS_ANYREF`] |
//! | exceptions / tags               | yes      | no    | [`SUPPORTS_TAGS`] |
//! | function-references             | yes      | no    | [`SUPPORTS_ANYREF`] |
//!
//! WASI preview1 is not a proposal but a host API, and it is opt-in rather than
//! configured — see [`link_wasi`] and [`SUPPORTS_WASI`].
//!
//! `function-references` has no JS-API spelling of its own — it rides along
//! with gc, which wasmtime requires it for — so it shares gc's constant.
//!
//! Threads is the one proposal turned *off* where the engine offers it: den
//! cannot represent a shared memory at the JS boundary (see
//! [`SUPPORTS_SHARED_MEMORY`]), so accepting `(memory 1 1 shared)` on wasmtime
//! only bought a module that instantiates on one build and not the other.
//!
//! SIMD is the other way round: `v128` is what LLVM emits for ordinary Rust and
//! C, so a build that rejects it rejects real programs. wasmi gates
//! `Config::wasm_simd` behind its `simd` cargo feature, which den's manifest
//! therefore enables.

#[cfg(all(feature = "wasmtime", feature = "wasmi"))]
compile_error!("den-stdlib-wasm: enable exactly one of the `wasmtime` or `wasmi` features");
#[cfg(not(any(feature = "wasmtime", feature = "wasmi")))]
compile_error!("den-stdlib-wasm: enable exactly one of the `wasmtime` or `wasmi` features");

#[cfg(feature = "wasmtime")] mod wasmtime;
#[cfg(feature = "wasmtime")]
pub use self::wasmtime::*;

#[cfg(all(feature = "wasmi", not(feature = "wasmtime")))]
mod wasmi;
use rquickjs::Ctx;

#[cfg(all(feature = "wasmi", not(feature = "wasmtime")))]
pub use self::wasmi::*;

/// The one import namespace [`link_wasi`] defines, and therefore the only one
/// `den:wasm`'s `wasiImports()` may be passed as.
pub const WASI_NAMESPACE: &str = "wasi_snapshot_preview1";

/// Whether `new WebAssembly.Memory({ shared: true })` can succeed.
///
/// `false` on *both* backends, and not a per-backend constant, because the
/// limit is den's rather than the engine's. §5.6 requires a shared memory's
/// `[[BufferObject]]` to be a `SharedArrayBuffer`, and den has no way to build
/// one that aliases the linear memory: `JS_NewArrayBuffer` with `is_shared`
/// set produces a buffer `JS_DetachArrayBuffer` silently refuses to detach
/// (quickjs.c:57837), which would turn the growth protocol in `memory.rs` into
/// a use-after-free. wasmtime cannot help either — it allocates shared
/// memories only through `SharedMemory` and `Memory::new` bails on a shared
/// type (wasmtime-48.0.0 src/runtime/memory.rs:303) — and wasmi has no threads
/// support at all.
///
/// So the constant is the honest answer, it makes both backends throw the same
/// `TypeError`, and `new_engine` derives `wasm_threads` from it so that no
/// module can smuggle a shared memory in past the JS API either.
pub const SUPPORTS_SHARED_MEMORY: bool = false;

/// Owning, lifetime-erased handle to the QuickJS context that created the
/// [`Store`].
///
/// wasmtime 48 bounds `T: 'static` on `Linker`/`Instance`/`Func`, so the store
/// payload cannot borrow `'js`. `Ctx` is refcounted (`Clone` is
/// `JS_DupContext`), so parking one keeps the `JSContext` alive and
/// [`OwnedCtx::with`] mints a callback-scoped `'js` on demand — `Ctx` is
/// invariant in `'js`, so it has to be minted rather than reborrowed.
///
/// This is the only `unsafe` in the crate's foundation; keep it that way.
///
/// Deliberately *not* `Sync`: nothing in either backend needs it (the store
/// payload is only ever reached through `&mut Store`, and the host closures
/// that must be `Send + Sync` capture no `OwnedCtx`), and asserting it would
/// make `StoreData` — hence `wasmtime::Store<StoreData>` — look shareable
/// between threads, which a `JSContext` is not.
pub struct OwnedCtx(Ctx<'static>);

impl OwnedCtx {
    pub fn new(ctx: &Ctx<'_>) -> Self {
        // SAFETY: `from_raw` takes a reference of its own via `JS_DupContext`, and the
        // caller is inside `ctx`, so the runtime lock is held right now.
        Self(unsafe { Ctx::from_raw(ctx.as_raw()) })
    }

    /// Re-narrow the erased context to a callback-scoped `'js`.
    ///
    /// This is the only way to reach the context: a `fn ctx(&self) -> Ctx<'_>`
    /// would hand out a lifetime the caller could outlive.
    pub fn with<R>(&self, f: impl FnOnce(&Ctx<'_>) -> R) -> R {
        // SAFETY: `self.0` holds a live reference to this context — `Ctx::from_raw`
        // performs `JS_DupContext`, and the runtime drops its userdata (hence this
        // value) before `JS_FreeRuntime` — and the runtime lock is held by whoever
        // called into wasm: the `Store` this lives in is userdata of that very
        // context, and a host callback is only entered from a JS call that holds the
        // lock for the whole closure. The minted `Ctx` never escapes.
        let ctx = unsafe { Ctx::from_raw(self.0.as_raw()) };
        f(&ctx)
    }
}

/// Payload of the single [`Store`] den keeps per JS context.
///
/// `'static` by construction — see [`OwnedCtx`]. The WASI context is created
/// lazily by [`link_wasi`], and only ever from there: building one inherits the
/// host's stdio and environment, which no context may be handed until a script
/// has asked for WASI by name.
pub struct StoreData {
    ctx:  OwnedCtx,
    wasi: Option<WasiCtx>,
}

impl StoreData {
    pub fn new(ctx: &Ctx<'_>) -> Self {
        Self {
            ctx:  OwnedCtx::new(ctx),
            wasi: None,
        }
    }

    /// The store's WASI context, built on first use by `init`.
    pub fn wasi_or_init(&mut self, init: impl FnOnce() -> WasiCtx) -> &mut WasiCtx {
        self.wasi.get_or_insert_with(init)
    }

    /// Run `f` with the JS context that owns this store.
    pub fn with_ctx<R>(&self, f: impl FnOnce(&Ctx<'_>) -> R) -> R {
        self.ctx.with(f)
    }
}

/// Backend-neutral discriminant of a [`ValType`].
///
/// Neither engine's `ValType` can be matched on directly by shared code:
/// wasmtime nests reference types in `ValType::Ref(RefType)` and derives
/// neither `Copy` nor `PartialEq`, wasmi has a flat enum that is `Copy +
/// PartialEq` but has no `anyref` at all.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ValKind {
    I32,
    I64,
    F32,
    F64,
    V128,
    FuncRef,
    ExternRef,
    AnyRef,
}

impl ValKind {
    /// The `WebAssembly.Global`/`Table` descriptor spelling of this type.
    pub const fn name(self) -> &'static str {
        match self {
            Self::I32 => "i32",
            Self::I64 => "i64",
            Self::F32 => "f32",
            Self::F64 => "f64",
            Self::V128 => "v128",
            Self::FuncRef => "anyfunc",
            Self::ExternRef => "externref",
            Self::AnyRef => "anyref",
        }
    }

    /// Inverse of [`ValKind::name`], accepting `"funcref"` as an alias of
    /// `"anyfunc"`.
    pub fn parse(name: &str) -> Option<Self> {
        Some(match name {
            "i32" => Self::I32,
            "i64" => Self::I64,
            "f32" => Self::F32,
            "f64" => Self::F64,
            "v128" => Self::V128,
            "anyfunc" | "funcref" => Self::FuncRef,
            "externref" => Self::ExternRef,
            "anyref" => Self::AnyRef,
            _ => return None,
        })
    }
}

/// Backend-neutral view of a [`Val`], so that JS conversion can be written
/// once.
///
/// Floats are real values here: wasmtime stores raw IEEE bits in
/// `Val::F32`/`F64` and wasmi stores `F32`/`F64` newtypes.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum ValView {
    I32(i32),
    I64(i64),
    F32(f32),
    F64(f64),
    V128,
    /// A null reference of any reference type.
    NullRef,
    /// A non-null reference; not representable in JS without the host value
    /// cache.
    Ref,
}

/// `"i32"`, `"anyfunc"`, … to the backend's own type, or `None` when this
/// backend cannot represent it (wasmi has no `anyref`).
pub fn val_type_from_str(name: &str) -> Option<ValType> {
    ValKind::parse(name).and_then(val_type_from_kind)
}

/// Descriptor spelling of a backend type, or `None` for types with no JS name
/// (wasmtime's GC reference types such as `structref`).
pub fn val_type_name(ty: &ValType) -> Option<&'static str> {
    val_type_kind(ty).map(ValKind::name)
}

/// The predicate shared code needs in place of `ValType::is_i64()` /
/// `is_v128()` (wasmtime-only) or `ValType == ValType` (wasmi-only).
pub fn val_type_is(ty: &ValType, kind: ValKind) -> bool {
    val_type_kind(ty) == Some(kind)
}

/// Same predicate against a global's content type, which the two engines return
/// differently (`&ValType` on wasmtime, `ValType` by value on wasmi).
pub fn global_content_is(ty: &GlobalType, kind: ValKind) -> bool {
    val_type_is(&global_content(ty), kind)
}

/// One JS program, run against whichever engine was compiled in.
///
/// A capability constant that only the code it gates ever reads proves
/// nothing: the same `#[cfg]` would pick both the behaviour and the
/// expectation, so the assertion can never fail. Everything below is therefore
/// either backend-*independent* — the error class a script sees — or a claim
/// about the engine checked against what the engine actually does with a
/// module. A constant that stops matching its backend fails here instead of in
/// somebody's script.
#[cfg(test)]
mod parity {
    use rquickjs::{Context, Module, Runtime};

    use super::{SUPPORTS_ANYREF, SUPPORTS_SHARED_MEMORY, SUPPORTS_TAGS, SUPPORTS_V128};

    /// Each entry is `"<what>: <outcome>"`, where the outcome is the value the
    /// operation produced or the `name` of the error it threw.
    const OBSERVE: &str = r#"
      const observations = [];
      const attempt = (f) => {
        try {
          return String(f());
        } catch (error) {
          return error.name;
        }
      };
      const observe = (what, f) => observations.push(`${what}: ${attempt(f)}`);
      const validates = (wat) => WebAssembly.validate(WebAssembly.wat2wasm(wat));
      const run = (wat, name, ...args) =>
        new WebAssembly.Instance(new WebAssembly.Module(WebAssembly.wat2wasm(wat)))
          .exports[name](...args);

      // Shared memory: refused the same way on both backends, at both the JS
      // API and the module level.
      observe("a shared memory", () => new WebAssembly.Memory({ initial: 1, maximum: 1, shared: true }));
      observe("a shared memory without a maximum", () => new WebAssembly.Memory({ initial: 1, shared: true }));
      observe("a module with a shared memory", () => validates(`(module (memory 1 1 shared))`));
      observe("a module using atomics", () =>
        validates(`(module (memory 1 1 shared) (func (result i32) i32.const 0 i32.atomic.load))`));

      // Value types with no JS representation, rejected by the shared
      // descriptor plumbing rather than by the engine.
      observe("an anyref global", () => new WebAssembly.Global({ value: "anyref" }, null));
      observe("a v128 global", () => new WebAssembly.Global({ value: "v128" }, 0));

      // Proposals only one engine implements: the answer differs, the constant
      // says which way.
      observe("a module using anyref", () => validates(`(module (func (result anyref) ref.null any))`));
      observe("a module using v128", () => validates(`(module (func (result v128) v128.const i32x4 0 0 0 0))`));
      observe("a module with a tag", () => validates(`(module (tag (param i32)))`));
      observe("a tag", () => new WebAssembly.Tag({ parameters: ["i32"] }) instanceof WebAssembly.Tag);

      // Proposals both engines are configured to accept, and one both refuse.
      observe("a module using tail calls", () => validates(`(module (func $f) (func (return_call $f)))`));
      observe("a module using extended const", () =>
        validates(`(module (global i32 (i32.add (i32.const 1) (i32.const 2))))`));
      observe("a module with two memories", () => validates(`(module (memory 1) (memory 1))`));
      observe("a module using memory64", () => validates(`(module (memory i64 1))`));
      observe("a module using custom page sizes", () => validates(`(module (memory 1 (pagesize 1)))`));
      observe("a module using bulk memory", () =>
        validates(`(module (memory 1) (func (memory.fill (i32.const 0) (i32.const 0) (i32.const 1))))`));

      // And the ordinary case, which must be identical everywhere.
      observe("an exported function", () =>
        run(`(module (func (export "add") (param i32 i32) (result i32)
               local.get 0 local.get 1 i32.add))`, "add", 1, 2));
      observe("a trapping export", () => run(`(module (func (export "boom") unreachable))`, "boom"));

      observations
    "#;

    #[test]
    fn both_backends_answer_the_same_program_the_same_way() {
        let runtime = Runtime::new().expect("runtime");
        let context = Context::full(&runtime).expect("context");
        context.with(|ctx| {
            let (_, evaluation) =
                Module::evaluate_def::<crate::js_wasm, _>(ctx.clone(), "den:wasm")
                    .expect("den:wasm evaluates");
            evaluation.finish::<()>().expect("den:wasm finishes");
            let observed: Vec<String> = ctx.eval(OBSERVE).expect("the program runs");

            let expected = [
                ("a shared memory", "TypeError".to_owned()),
                ("a shared memory without a maximum", "TypeError".to_owned()),
                (
                    "a module with a shared memory",
                    SUPPORTS_SHARED_MEMORY.to_string(),
                ),
                ("a module using atomics", SUPPORTS_SHARED_MEMORY.to_string()),
                ("an anyref global", "TypeError".to_owned()),
                ("a v128 global", "TypeError".to_owned()),
                ("a module using anyref", SUPPORTS_ANYREF.to_string()),
                ("a module using v128", SUPPORTS_V128.to_string()),
                ("a module with a tag", SUPPORTS_TAGS.to_string()),
                // The constructor reports the same lack as a `TypeError`, per §5.9.
                (
                    "a tag",
                    if SUPPORTS_TAGS { "true" } else { "TypeError" }.to_owned(),
                ),
                ("a module using tail calls", "true".to_owned()),
                ("a module using extended const", "true".to_owned()),
                ("a module with two memories", "true".to_owned()),
                ("a module using memory64", "true".to_owned()),
                ("a module using custom page sizes", "false".to_owned()),
                ("a module using bulk memory", "true".to_owned()),
                ("an exported function", "3".to_owned()),
                ("a trapping export", "RuntimeError".to_owned()),
            ]
            .map(|(what, outcome)| format!("{what}: {outcome}"));

            assert_eq!(observed, expected.to_vec());
        })
    }
}
