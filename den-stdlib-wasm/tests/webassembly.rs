//! The WebAssembly JS API driven through the real [`Engine`].
//!
//! `den-stdlib-wasm` proves the semantics against a bare `AsyncContext`; what
//! is proved here is that a user who calls `Engine::eval` gets those semantics
//! — module registration, the transpiler in front of the source and the
//! userdata wiring all included. Every assertion travels back into Rust, so a
//! JS-side failure cannot pass as green.

use color_eyre::eyre;
use den_core::engine::Engine;
use rquickjs::FromJs;

/// Exports covering the plain call path, plus a custom section for the
/// `Module.customSections` static.
const ADD: &str = r#"
    (module
      (@custom "greeting" "hello")
      (func (export "add") (param i32 i32) (result i32) local.get 0 local.get 1 i32.add)
      (func (export "boom") unreachable))
"#;

const CALLS_IMPORT: &str = r#"
    (module
      (import "env" "log" (func $log (param i32 i32) (result i32)))
      (func (export "run") (param i32 i32) (result i32)
        local.get 0 local.get 1 call $log))
"#;

const I64: &str = r#"
    (module
      (func (export "echo") (param i64) (result i64) local.get 0)
      (func (export "beyond") (result i64) i64.const 9007199254740993))
"#;

const MEMORY: &str = r#"
    (module
      (memory (export "mem") 1)
      (func (export "peek") (param i32) (result i32) local.get 0 i32.load8_u))
"#;

/// An export that calls back into JS, which calls the export again: the store
/// is already mutably borrowed, so the inner call is answered with a
/// `RuntimeError`.
const REENTRANT: &str = r#"
    (module
      (import "env" "reenter" (func $reenter))
      (func (export "run") call $reenter))
"#;

const TABLE_AND_GLOBAL: &str = r#"
    (module
      (table (export "table") 1 funcref)
      (global (export "counter") (mut i32) (i32.const 7)))
"#;

/// Evaluate `body` in a fresh engine with `WASM` bound to the assembled bytes
/// of `wat`. One engine per call: `den:wasm` keeps a single store in the
/// context userdata, so tests must not share one.
async fn eval<T>(wat: &str, body: &str) -> eyre::Result<T>
where
    T: for<'js> FromJs<'js> + Send + Sync + 'static,
{
    let engine = Engine::new().await;
    Ok(engine
        .eval(&format!(
            "const {{ wat2wasm }} = await import('den:wasm');\nconst WASM = \
             wat2wasm(`{wat}`);\n{body}"
        ))
        .await?)
}

/// Regression test for a heap corruption that took the whole process down: a
/// `Vec` den built in Rust and lent to QuickJS through `ArrayBuffer::new` had
/// its free hook run twice (quickjs.c:58037 and :57935), and
/// `ArrayBuffer.prototype.transfer` handed `js_realloc` a pointer `js_malloc`
/// never produced. `wat2wasm` is the shortest route to such a buffer, and the
/// symptom was a SIGABRT — so this test surviving at all is most of the
/// assertion; the rest pins that the transfer did what it says.
#[tokio::test(flavor = "multi_thread")]
async fn a_rust_built_buffer_survives_being_transferred() -> eyre::Result<()> {
    let failures: String = eval(
        "(module)",
        r#"
          const original = WASM.buffer;
          const size = original.byteLength;
          const first = new Uint8Array(WASM)[0];
          const moved = original.transfer(4);
          // The bytes are re-read after the realloc, and a second buffer is
          // built and dropped, so a corrupted allocator has every chance to
          // notice before the assertions do.
          const copy = new Uint8Array(moved).join("-");
          new ArrayBuffer(1024).transfer(2048);
          Object.entries({
            theModuleWasAssembled: size >= 8,
            theMagicByteWasThere: first === 0x00,
            theSourceIsDetached: original.detached === true,
            theDestinationHasTheAskedForSize: moved.byteLength === 4,
            theFirstFourBytesTravelled: copy === "0-97-115-109",
          }).filter(([, held]) => !held).map(([name]) => name).join(",")
        "#,
    )
    .await?;
    assert_eq!(failures, "");
    Ok(())
}

/// den's `wat2wasm` assembles every fixture below, so pin it against the `wat`
/// crate itself once — a broken assembler would make the rest of the file
/// vacuous.
#[tokio::test(flavor = "multi_thread")]
async fn wat2wasm_assembles_the_same_bytes_as_the_reference_assembler() -> eyre::Result<()> {
    let assembled: Vec<u8> = eval(ADD, "Array.from(WASM)").await?;
    assert_eq!(assembled, wat::parse_str(ADD)?);
    Ok(())
}

#[tokio::test(flavor = "multi_thread")]
async fn validate_answers_true_for_a_real_module_and_false_for_garbage() -> eyre::Result<()> {
    let answers: String = eval(
        ADD,
        r#"[WebAssembly.validate(WASM), WebAssembly.validate(new Uint8Array([1, 2, 3]))].join(",")"#,
    )
    .await?;
    assert_eq!(answers, "true,false");
    Ok(())
}

#[tokio::test(flavor = "multi_thread")]
async fn instantiating_a_buffer_source_yields_a_module_instance_pair() -> eyre::Result<()> {
    let sum: i32 = eval(
        ADD,
        r#"
          const result = await WebAssembly.instantiate(WASM);
          if (!(result.module instanceof WebAssembly.Module)) throw new Error("no module");
          if (!(result.instance instanceof WebAssembly.Instance)) throw new Error("no instance");
          result.instance.exports.add(20, 22)
        "#,
    )
    .await?;
    assert_eq!(sum, 42);
    Ok(())
}

/// The other overload of the same function: a compiled `Module` resolves with a
/// bare `Instance`, not with a pair.
#[tokio::test(flavor = "multi_thread")]
async fn instantiating_a_compiled_module_yields_a_bare_instance() -> eyre::Result<()> {
    let shape: String = eval(
        ADD,
        r#"
          const instance = await WebAssembly.instantiate(await WebAssembly.compile(WASM));
          [instance instanceof WebAssembly.Instance,
           instance.module === undefined,
           instance.exports.add(1, 2) === 3].join(",")
        "#,
    )
    .await?;
    assert_eq!(shape, "true,true,true");
    Ok(())
}

#[tokio::test(flavor = "multi_thread")]
async fn an_imported_js_function_receives_the_wasm_arguments() -> eyre::Result<()> {
    let outcome: String = eval(
        CALLS_IMPORT,
        r#"
          let seen = null;
          const { instance } = await WebAssembly.instantiate(WASM, {
            env: { log: (left, right) => { seen = [left, right]; return left * right; } },
          });
          `${instance.exports.run(6, 7)}|${seen.join(",")}`
        "#,
    )
    .await?;
    assert_eq!(outcome, "42|6,7");
    Ok(())
}

/// `i64` is the one wasm type that has to cross as a `BigInt`: a Number cannot
/// carry it losslessly, so the conversion rejects Numbers on the way in.
#[tokio::test(flavor = "multi_thread")]
async fn i64_exports_and_parameters_cross_the_boundary_as_bigint() -> eyre::Result<()> {
    let failures: String = eval(
        I64,
        r#"
          const { echo, beyond } = (await WebAssembly.instantiate(WASM)).instance.exports;
          let numberRejected = false;
          try { echo(1); } catch (error) { numberRejected = error instanceof TypeError; }
          Object.entries({
            resultIsBigInt: typeof beyond() === "bigint",
            resultIsExact: beyond() === 9007199254740993n,
            argumentRoundTrips: echo(-9007199254740993n) === -9007199254740993n,
            numberRejected,
          }).filter(([, held]) => !held).map(([name]) => name).join(",")
        "#,
    )
    .await?;
    assert_eq!(failures, "");
    Ok(())
}

/// The `ArrayBuffer` aliases the linear memory, and `grow` may relocate it —
/// hence the detach, which is the only thing standing between JS and freed
/// pages.
#[tokio::test(flavor = "multi_thread")]
async fn memory_is_shared_with_wasm_and_grow_detaches_the_previous_buffer() -> eyre::Result<()> {
    let failures: String = eval(
        MEMORY,
        r#"
          const { mem, peek } = (await WebAssembly.instantiate(WASM)).instance.exports;
          const stale = mem.buffer;
          const sizeBeforeGrow = stale.byteLength;
          new Uint8Array(stale)[3] = 42;
          const seenByWasm = peek(3);
          const previousPages = mem.grow(1);
          Object.entries({
            memoryIsAMemory: mem instanceof WebAssembly.Memory,
            initialSizeIsOnePage: sizeBeforeGrow === 65536,
            wasmSeesTheJsWrite: seenByWasm === 42,
            growReturnsThePreviousSize: previousPages === 1,
            staleBufferIsDetached: stale.byteLength === 0,
            freshBufferIsHandedOut: mem.buffer !== stale,
            freshBufferIsTwoPages: mem.buffer.byteLength === 131072,
          }).filter(([, held]) => !held).map(([name]) => name).join(",")
        "#,
    )
    .await?;
    assert_eq!(failures, "");
    Ok(())
}

/// Non-null references are not representable in JS yet (there is no host value
/// cache), so `null` is the only element a `Table` accepts — asserted here so
/// the day that changes, this test says so.
#[tokio::test(flavor = "multi_thread")]
async fn table_and_global_are_readable_and_writable_from_js() -> eyre::Result<()> {
    let failures: String = eval(
        TABLE_AND_GLOBAL,
        r#"
          const { table, counter } = (await WebAssembly.instantiate(WASM)).instance.exports;
          const lengthBeforeGrow = table.length;
          const previousLength = table.grow(2);
          table.set(0, null);
          let outOfRange = false;
          try { table.get(99); } catch (error) { outOfRange = error instanceof RangeError; }
          counter.value = 11;
          Object.entries({
            tableIsATable: table instanceof WebAssembly.Table,
            initialLengthIsOne: lengthBeforeGrow === 1,
            growReturnsThePreviousLength: previousLength === 1,
            lengthReflectsTheGrowth: table.length === 3,
            uninitialisedSlotIsNull: table.get(0) === null,
            outOfRangeGetThrowsRangeError: outOfRange,
            globalIsAGlobal: counter instanceof WebAssembly.Global,
            globalSetterTakesEffect: counter.value === 11,
          }).filter(([, held]) => !held).map(([name]) => name).join(",")
        "#,
    )
    .await?;
    assert_eq!(failures, "");
    Ok(())
}

/// Each error class must come from the operation the spec assigns it to, and
/// all three must be real `Error` subclasses.
///
/// `RuntimeError` is raised here by a re-entrant call;
/// `a_trapping_export_should_reject_with_a_runtime_error` below covers the trap
/// path, which reaches the same class by a different route.
#[tokio::test(flavor = "multi_thread")]
async fn each_webassembly_error_class_is_thrown_by_its_own_operation() -> eyre::Result<()> {
    let thrown: String = eval(
        REENTRANT,
        r#"
          const caught = async (thunk) => {
            try { await thunk(); return "nothing thrown"; }
            catch (error) { return `${error.name}/${error instanceof Error}`; }
          };
          const tooSmall = wat2wasm('(module (import "env" "mem" (memory 2)))');
          let reentrant = "not reached";
          const { instance } = await WebAssembly.instantiate(WASM, {
            env: { reenter: () => { reentrant = "nothing thrown"; try { instance.exports.run(); }
                                    catch (error) { reentrant = `${error.name}/${error instanceof Error}`; } } },
          });
          [
            await caught(() => WebAssembly.compile(new Uint8Array([0, 97, 115, 109, 9, 9, 9, 9]))),
            await caught(() => WebAssembly.instantiate(tooSmall, {
              env: { mem: new WebAssembly.Memory({ initial: 1 }) },
            })),
            (instance.exports.run(), reentrant),
          ].join("|")
        "#,
    )
    .await?;
    assert_eq!(thrown, "CompileError/true|LinkError/true|RuntimeError/true");
    Ok(())
}

/// A trapping export must reject with `WebAssembly.RuntimeError`.
///
/// This pins both halves of what used to break it.
/// `Instance::throw_call_failure` prefers a still-pending QuickJS exception
/// over the trap description, which is right only if "pending" is decided by
/// `Ctx::has_exception`: `Ctx::catch` is `JS_GetException`, which answers
/// `JS_UNINITIALIZED` when nothing is pending, and re-throwing that sentinel
/// segfaults QuickJS as soon as JS reads `.name` off it. And a discarded
/// conversion probe — `BufferSource::from_js` trying `ArrayBuffer` before the
/// `ArrayBufferView` path — used to leave its own `TypeError` pending
/// forever, so every later trap in the context reported *that* error instead.
#[tokio::test(flavor = "multi_thread")]
async fn a_trapping_export_should_reject_with_a_runtime_error() -> eyre::Result<()> {
    let thrown: String = eval(
        ADD,
        r#"
          const { boom } = (await WebAssembly.instantiate(WASM)).instance.exports;
          let thrown = "nothing thrown";
          try { boom(); } catch (error) { thrown = `${error.name}/${error instanceof Error}`; }
          thrown
        "#,
    )
    .await?;
    assert_eq!(thrown, "RuntimeError/true");
    Ok(())
}

#[tokio::test(flavor = "multi_thread")]
async fn module_statics_describe_imports_exports_and_custom_sections() -> eyre::Result<()> {
    let described: String = eval(
        ADD,
        r#"
          const module = await WebAssembly.compile(WASM);
          const greeting = WebAssembly.Module.customSections(module, "greeting");
          const missing = WebAssembly.Module.customSections(module, "absent");
          JSON.stringify({
            imports: WebAssembly.Module.imports(module),
            exports: WebAssembly.Module.exports(module),
            greeting: String.fromCharCode(...new Uint8Array(greeting[0])),
            greetingIsArrayBuffer: greeting[0] instanceof ArrayBuffer,
            absent: missing.length,
          })
        "#,
    )
    .await?;
    assert_eq!(
        described,
        r#"{"imports":[],"exports":[{"name":"add","kind":"function"},{"name":"boom","kind":"function"}],"greeting":"hello","greetingIsArrayBuffer":true,"absent":0}"#
    );
    Ok(())
}

#[tokio::test(flavor = "multi_thread")]
async fn the_exports_object_is_frozen_with_a_null_prototype() -> eyre::Result<()> {
    let shape: String = eval(
        ADD,
        r#"
          const { exports } = (await WebAssembly.instantiate(WASM)).instance;
          [Object.isFrozen(exports),
           Object.getPrototypeOf(exports) === null,
           exports === (await WebAssembly.instantiate(WASM)).instance.exports].join(",")
        "#,
    )
    .await?;
    assert_eq!(shape, "true,true,false");
    Ok(())
}
