//! `WebAssembly.Module`.

use std::sync::Arc;

use indexmap::{IndexMap, indexmap};
use rquickjs::{
    ArrayBuffer, Ctx, Exception, FromJs, Function, JsLifetime, Object, Result, Value, class::Trace,
};

use crate::{Probe, backend, engine::Engine, error::throw_compile_error};

/// A wasm binary is `magic(4) || version(4)` followed by sections.
const WASM_HEADER_LEN: usize = 8;
const CUSTOM_SECTION_ID: u8 = 0;
const IMPORT_SECTION_ID: u8 = 2;
const EXPORT_SECTION_ID: u8 = 7;
/// `exportdesc` tag of a function export.
const FUNCTION_EXPORT_KIND: u8 = 0;
/// `importdesc` tags.
const FUNCTION_IMPORT_KIND: u8 = 0;
const TABLE_IMPORT_KIND: u8 = 1;
const MEMORY_IMPORT_KIND: u8 = 2;
const GLOBAL_IMPORT_KIND: u8 = 3;
const TAG_IMPORT_KIND: u8 = 4;
/// `(ref ht)` and `(ref null ht)`: the only value types carrying an operand.
const REFERENCE_TYPE_PREFIXES: [u8; 2] = [0x64, 0x63];
/// `limits` flags: bit 0 carries a maximum, bit 3 a custom page size.
const LIMITS_HAS_MAXIMUM: u8 = 0x01;
const LIMITS_HAS_PAGE_SIZE: u8 = 0x08;
const LEB_CONTINUATION_BIT: u8 = 0x80;
const LEB_PAYLOAD_MASK: u8 = 0x7f;
const LEB_PAYLOAD_BITS: u32 = 7;
/// A `u32` LEB128 never needs more than ⌈32/7⌉ bytes.
const LEB_MAX_BYTES: u32 = 5;
/// …and a `u64` one — the width of a memory64 limit — no more than ⌈64/7⌉.
const LEB_MAX_BYTES_64: u32 = 10;

/// `[AllowShared] BufferSource` — the module bytes, copied out of the JS
/// buffer.
///
/// The spec copies first ("let stableBytes be a copy of the bytes held by the
/// buffer") precisely so that a `SharedArrayBuffer` mutated by another agent
/// cannot be observed half-compiled.
pub struct BufferSource(Vec<u8>);

impl BufferSource {
    pub fn bytes(&self) -> &[u8] {
        &self.0
    }

    pub fn into_bytes(self) -> Vec<u8> {
        self.0
    }

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
    pub(crate) inner: backend::Module,
    /// `[[Bytes]]`. Neither engine keeps the original binary, but the JS API
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

    /// The `(id, payload)` of every section of `[[Bytes]]`, stopping at the
    /// first malformed one. The bytes come from a module that compiled, so
    /// that can only happen if a section length is truncated.
    fn sections<'a>(&'a self) -> impl Iterator<Item = (u8, &'a [u8])> + 'a {
        let mut reader = SectionReader::new(self.bytes.get(WASM_HEADER_LEN..).unwrap_or_default());
        core::iter::from_fn(move || reader.section())
    }

    /// The payloads of the custom sections named `wanted`, in module order.
    fn custom_section_payloads<'a>(&'a self, wanted: &str) -> Vec<&'a [u8]> {
        self.sections()
            .filter(|(id, _)| *id == CUSTOM_SECTION_ID)
            .filter_map(|(_, payload)| {
                let mut reader = SectionReader::new(payload);
                (reader.name()? == wanted).then_some(reader.rest)
            })
            .collect()
    }

    /// The entries of the first section with this `id`, each read by `entry`,
    /// stopping at the first one that does not parse.
    fn section_entries<'a, T>(
        &'a self,
        id: u8,
        mut entry: impl FnMut(&mut SectionReader<'a>) -> Option<T>,
    ) -> Vec<T> {
        let Some((_, payload)) = self.sections().find(|(section, _)| *section == id) else {
            return Vec::new();
        };
        let mut reader = SectionReader::new(payload);
        let count = reader.leb_u32().unwrap_or_default();
        (0..count).map_while(|_| entry(&mut reader)).collect()
    }

    /// `(name, exportdesc tag, index)` of every export, in declaration order.
    fn export_entries(&self) -> Vec<(&str, u8, u32)> {
        self.section_entries(EXPORT_SECTION_ID, |reader| {
            Some((reader.name()?, reader.byte()?, reader.leb_u32()?))
        })
    }

    /// `export name → function index`, read out of the export section because
    /// an Exported Function is named after its index in the module and neither
    /// engine exposes that.
    pub(crate) fn exported_function_indices(&self) -> IndexMap<&str, u32> {
        self.export_entries()
            .into_iter()
            .filter(|(_, kind, _)| *kind == FUNCTION_EXPORT_KIND)
            .map(|(name, _, index)| (name, index))
            .collect()
    }

    /// Every export name, in the order the binary declares it.
    pub(crate) fn declared_exports(&self) -> Vec<&str> {
        self.export_entries()
            .into_iter()
            .map(|(name, _, _)| name)
            .collect()
    }

    /// The `(module, name)` of every import, in the order the binary declares
    /// it.
    pub(crate) fn declared_imports(&self) -> Vec<(&str, &str)> {
        self.section_entries(IMPORT_SECTION_ID, |reader| {
            let entry = (reader.name()?, reader.name()?);
            reader.skip_import_desc()?;
            Some(entry)
        })
    }

    /// Permute `items` — an engine's import or export list — into the order
    /// `declared` gives.
    ///
    /// The JS API observes both orders: `Object.keys(instance.exports)` and the
    /// sequence of `Get`s against the import object are the module's own
    /// declaration order. wasmtime already reports that order, but wasmi groups
    /// imports by kind (`ModuleImportsBuilder::finish` chains funcs, tables,
    /// memories then globals) and keeps exports in a sorted `Map`, so the same
    /// program would otherwise behave differently depending on which backend
    /// den was built with. Only `[[Bytes]]` — retained for `customSections`
    /// anyway — still has the real order.
    ///
    /// Whatever `declared` does not account for keeps the engine's own order at
    /// the end, so a binary this reader cannot follow degrades to "unordered",
    /// never to "lost".
    pub(crate) fn in_declaration_order<T, K: PartialEq>(
        declared: &[K],
        items: Vec<T>,
        key: impl Fn(&T) -> K,
    ) -> Vec<T> {
        // ponytail: linear scan per entry, because an import may legally repeat a
        // `(module, name)` pair and a map would lose one. Both lists are tens of
        // entries at most; an index would only pay off for generated giants.
        let mut remaining: Vec<Option<T>> = items.into_iter().map(Some).collect();
        let mut ordered = Vec::with_capacity(remaining.len());
        for wanted in declared {
            let found = remaining
                .iter()
                .position(|item| item.as_ref().is_some_and(|item| key(item) == *wanted));
            if let Some(position) = found {
                ordered.extend(remaining[position].take());
            }
        }
        ordered.extend(remaining.into_iter().flatten());
        ordered
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
        Self::in_declaration_order(
            &module.declared_imports(),
            module.inner.imports().collect(),
            |import| (import.module(), import.name()),
        )
        .into_iter()
        .map(|import| {
            // Bound rather than borrowed at the call: one backend hands out a
            // reference to the extern type and the other an owned one, so the binding
            // is what makes `&ExternType` the type either way.
            let ty = &import.ty();
            indexmap! {
                "module" => import.module().to_owned(),
                "name" => import.name().to_owned(),
                "kind" => backend::extern_kind_name(ty).to_owned(),
            }
        })
        .collect()
    }

    #[qjs(static)]
    pub fn exports(module: &Module) -> Vec<IndexMap<&'static str, String>> {
        Self::in_declaration_order(
            &module.declared_exports(),
            module.inner.exports().collect(),
            |export| export.name(),
        )
        .into_iter()
        .map(|export| {
            // Bound rather than borrowed at the call, for the same reason as in
            // `imports` above.
            let ty = &export.ty();
            indexmap! {
                "name" => export.name().to_owned(),
                "kind" => backend::extern_kind_name(ty).to_owned(),
            }
        })
        .collect()
    }

    #[qjs(static)]
    pub fn custom_sections<'js>(
        module: &Module,
        section_name: String,
        ctx: Ctx<'js>,
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

/// Cursor over a wasm binary, or over the payload of one of its sections.
///
/// Everything is `Option`-returning: the input is JS-controlled, so running off
/// the end must end the walk, never panic.
struct SectionReader<'a> {
    rest: &'a [u8],
}

impl<'a> SectionReader<'a> {
    fn new(bytes: &'a [u8]) -> Self {
        Self { rest: bytes }
    }

    fn take(&mut self, count: usize) -> Option<&'a [u8]> {
        let (head, tail) = self.rest.split_at_checked(count)?;
        self.rest = tail;
        Some(head)
    }

    fn byte(&mut self) -> Option<u8> {
        self.take(1)?.first().copied()
    }

    fn leb_u32(&mut self) -> Option<u32> {
        let mut value = 0;
        for index in 0..LEB_MAX_BYTES {
            let byte = self.byte()?;
            value |= u32::from(byte & LEB_PAYLOAD_MASK) << (index * LEB_PAYLOAD_BITS);
            if byte & LEB_CONTINUATION_BIT == 0 {
                return Some(value);
            }
        }
        // More continuation bytes than a `u32` can hold: the binary is corrupt.
        None
    }

    /// Consume a LEB128 integer of up to 64 bits without decoding it.
    fn skip_leb(&mut self) -> Option<()> {
        for _ in 0..LEB_MAX_BYTES_64 {
            if self.byte()? & LEB_CONTINUATION_BIT == 0 {
                return Some(());
            }
        }
        None
    }

    /// A `valtype`/`reftype`: one byte, plus a heap type index for the
    /// `(ref null? ht)` forms the GC proposal adds.
    fn skip_val_type(&mut self) -> Option<()> {
        match self.byte()? {
            prefix if REFERENCE_TYPE_PREFIXES.contains(&prefix) => self.skip_leb(),
            _ => Some(()),
        }
    }

    /// `limits`: a flags byte, the minimum, then whatever the flags announce.
    fn skip_limits(&mut self) -> Option<()> {
        let flags = self.byte()?;
        self.skip_leb()?;
        if flags & LIMITS_HAS_MAXIMUM != 0 {
            self.skip_leb()?;
        }
        if flags & LIMITS_HAS_PAGE_SIZE != 0 {
            self.skip_leb()?;
        }
        Some(())
    }

    /// `importdesc`, skipped rather than decoded: the engine already knows
    /// every import's type, so only the order of the names around it is
    /// wanted here.
    fn skip_import_desc(&mut self) -> Option<()> {
        match self.byte()? {
            FUNCTION_IMPORT_KIND => self.skip_leb(),
            TABLE_IMPORT_KIND => {
                self.skip_val_type()?;
                self.skip_limits()
            }
            MEMORY_IMPORT_KIND => self.skip_limits(),
            GLOBAL_IMPORT_KIND => {
                self.skip_val_type()?;
                self.byte().map(drop)
            }
            // `tagdesc` is an attribute byte followed by a type index.
            TAG_IMPORT_KIND => {
                self.byte()?;
                self.skip_leb()
            }
            _ => None,
        }
    }

    /// A `name` is a length-prefixed UTF-8 string.
    fn name(&mut self) -> Option<&'a str> {
        let length = self.leb_u32()?;
        core::str::from_utf8(self.take(length as usize)?).ok()
    }

    fn section(&mut self) -> Option<(u8, &'a [u8])> {
        let id = self.byte()?;
        let length = self.leb_u32()?;
        Some((id, self.take(length as usize)?))
    }
}
