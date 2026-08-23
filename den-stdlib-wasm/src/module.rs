//! `WebAssembly.Module`.

use std::sync::Arc;

use indexmap::{IndexMap, indexmap};
use rquickjs::{
    ArrayBuffer, Ctx, Exception, FromJs, Function, JsLifetime, Object, Result, Value, class::Trace,
};
use wasmtime::{
    Module as WasmModule,
    wasmparser::{ExternalKind, Parser, Payload},
};

use crate::{Probe, backend, engine::Engine, error::throw_compile_error};

/// `[AllowShared] BufferSource` — the module bytes, copied out of the JS
/// buffer.
///
/// The spec copies first ("let stableBytes be a copy of the bytes held by the
/// buffer") precisely so that a `SharedArrayBuffer` mutated by another agent
/// cannot be observed half-compiled.
pub struct BufferSource(Vec<u8>);

impl BufferSource {
    pub fn bytes(&self) -> &[u8] { &self.0 }

    pub fn into_bytes(self) -> Vec<u8> { self.0 }

    /// Any `ArrayBufferView` — `Uint8Array`, `DataView`, `Float64Array`, … —
    /// exposes its window over a buffer through these three properties, so
    /// there is no need to enumerate the view types.
    ///
    /// `ArrayBuffer.isView` is the gate rather than the three properties
    /// themselves: WebIDL's `BufferSource` is `ArrayBuffer or ArrayBufferView`
    /// and nothing else, so a plain object wearing the right property names is
    /// a `TypeError`, not a module.
    fn view_bytes<'js>(ctx: &Ctx<'js>, value: &Value<'js>) -> Result<Vec<u8>> {
        let type_error = || Exception::throw_type(ctx, "expected a BufferSource");
        let view = value.as_object().ok_or_else(type_error)?;
        if !Self::is_array_buffer_view(ctx, value)? {
            return Err(type_error());
        }
        let buffer: ArrayBuffer<'js> = view.get("buffer").map_err(|_| type_error())?;
        let offset: usize = view.get("byteOffset").map_err(|_| type_error())?;
        let length: usize = view.get("byteLength").map_err(|_| type_error())?;
        let bytes = buffer
            .as_bytes()
            .ok_or_else(|| Exception::throw_type(ctx, "the buffer is detached"))?;
        bytes
            .get(offset..offset.saturating_add(length))
            .map(<[u8]>::to_vec)
            .ok_or_else(|| Exception::throw_type(ctx, "the view is out of bounds of its buffer"))
    }

    /// `ArrayBuffer.isView` — the one brand check that covers every typed array
    /// and `DataView` without enumerating them, and that no ordinary object can
    /// forge.
    fn is_array_buffer_view<'js>(ctx: &Ctx<'js>, value: &Value<'js>) -> Result<bool> {
        ctx.globals()
            .get::<_, Object<'js>>("ArrayBuffer")?
            .get::<_, Function<'js>>("isView")?
            .call((value.clone(),))
    }
}

impl<'js> FromJs<'js> for BufferSource {
    fn from_js(ctx: &Ctx<'js>, value: Value<'js>) -> Result<Self> {
        // An `ArrayBuffer` and an `ArrayBufferView` are told apart by probing, so
        // the probe must not leave its `TypeError` pending for the view path — and
        // the plain `new WebAssembly.Module(uint8Array)` call *is* the view path.
        match ctx.probe(|| ArrayBuffer::from_js(ctx, value.clone()).ok()) {
            Some(buffer) => {
                buffer
                    .as_bytes()
                    .map(<[u8]>::to_vec)
                    .map(Self)
                    .ok_or_else(|| Exception::throw_type(ctx, "the buffer is detached"))
            }
            None => Self::view_bytes(ctx, &value).map(Self),
        }
    }
}

#[derive(Trace, JsLifetime, Clone)]
#[rquickjs::class]
pub struct Module {
    #[qjs(skip_trace)]
    pub(crate) inner: WasmModule,
    /// `[[Bytes]]`. wasmtime does not keep the original binary, but the JS API
    /// needs it for `customSections` and for the index an Exported Function is
    /// named after.
    #[qjs(skip_trace)]
    bytes:            Arc<[u8]>,
}

impl Module {
    /// Compile, mapping any decode or validation failure to `CompileError` as
    /// the JS API requires.
    pub fn compile(ctx: &Ctx<'_>, bytes: Vec<u8>) -> Result<Self> {
        let engine = Engine::from_ctx(ctx)?;
        let inner = backend::compile_module(&engine, &bytes)
            .map_err(|err| throw_compile_error(ctx, err))?;
        Ok(Self {
            inner,
            bytes: Arc::from(bytes),
        })
    }

    /// The payloads of the custom sections named `wanted`, in module order.
    fn custom_section_payloads<'a>(&'a self, wanted: &str) -> Vec<&'a [u8]> {
        Parser::new(0)
            .parse_all(&self.bytes)
            .filter_map(core::result::Result::ok)
            .filter_map(|payload| {
                match payload {
                    Payload::CustomSection(section) if section.name() == wanted => {
                        Some(section.data())
                    }
                    _ => None,
                }
            })
            .collect()
    }

    /// `export name → function index`, read out of the export section because
    /// an Exported Function is named after its index in the module and
    /// wasmtime does not expose that.
    pub(crate) fn exported_function_indices(&self) -> IndexMap<&str, u32> {
        Parser::new(0)
            .parse_all(&self.bytes)
            .filter_map(core::result::Result::ok)
            .find_map(|payload| {
                match payload {
                    Payload::ExportSection(reader) => Some(reader),
                    _ => None,
                }
            })
            .into_iter()
            .flatten()
            .filter_map(core::result::Result::ok)
            .filter(|export| export.kind == ExternalKind::Func)
            .map(|export| (export.name, export.index))
            .collect()
    }
}

#[rquickjs::methods(rename_all = "camelCase")]
impl Module {
    #[qjs(constructor)]
    pub fn new(bytes: BufferSource, ctx: Ctx<'_>) -> Result<Self> {
        Self::compile(&ctx, bytes.into_bytes())
    }

    #[qjs(static)]
    pub fn imports(module: &Module) -> Vec<IndexMap<&'static str, String>> {
        module
            .inner
            .imports()
            .map(|import| {
                indexmap! {
                    "module" => import.module().to_owned(),
                    "name" => import.name().to_owned(),
                    "kind" => backend::extern_kind_name(&import.ty()).to_owned(),
                }
            })
            .collect()
    }

    #[qjs(static)]
    pub fn exports(module: &Module) -> Vec<IndexMap<&'static str, String>> {
        module
            .inner
            .exports()
            .map(|export| {
                indexmap! {
                    "name" => export.name().to_owned(),
                    "kind" => backend::extern_kind_name(&export.ty()).to_owned(),
                }
            })
            .collect()
    }

    #[qjs(static)]
    pub fn custom_sections<'js>(
        module: &Module, section_name: String, ctx: Ctx<'js>,
    ) -> Result<Vec<ArrayBuffer<'js>>> {
        module
            .custom_section_payloads(&section_name)
            .into_iter()
            // `new_copy`, never `new`: `new` hands QuickJS a Rust `Vec`'s
            // backing store plus a free hook, and QuickJS runs that hook twice
            // on detach (quickjs.c:58037 and :57935) while `transfer` reallocs
            // a pointer its allocator never produced. `new_copy` allocates
            // through QuickJS, so the result is an ordinary JS buffer that
            // script may detach and transfer at will (see memory.rs `alias`).
            .map(|payload| ArrayBuffer::new_copy(ctx.clone(), payload))
            .collect()
    }
}
