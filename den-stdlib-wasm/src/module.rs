//! `WebAssembly.Module`.

use std::sync::Arc;

use den_util::BufferSource;
use indexmap::{IndexMap, indexmap};
use rquickjs::{
    ArrayBuffer, Coerced, Ctx, Error, JsLifetime, Result, String as JsString, class::Trace,
};
use wasmtime::{
    Module as WasmModule,
    wasmparser::{ExternalKind, Parser, Payload},
};

use crate::{backend, engine::Engine, error::throw_compile};

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
        let inner =
            backend::compile_module(&engine, &bytes).map_err(|err| throw_compile(ctx, err))?;
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
    /// `[AllowShared]` per the spec: [`BufferSource`] copies the bytes before
    /// compilation, so a `SharedArrayBuffer` mutated by another agent cannot
    /// be observed half-compiled.
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
        module: &Module, section_name: Coerced<JsString<'js>>, ctx: Ctx<'js>,
    ) -> Result<Vec<ArrayBuffer<'js>>> {
        // Wasm section names are valid UTF-8, so a JS string containing an
        // unpaired UTF-16 surrogate cannot match one. Do not fold it onto
        // U+FFFD, which is a valid and distinct section name.
        let section_name = match section_name.0.to_string() {
            Ok(section_name) => section_name,
            Err(Error::Utf8(_)) => return Ok(Vec::new()),
            Err(error) => return Err(error),
        };
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
