//! The schema — plain data, read once at `open()`.
//!
//! `docs/research/19-den-ffi.md` §3.1: the same value types the call site in
//! `types/den-ffi.d.ts`, drives the CIF here, and is validated at the
//! boundary. Anything den cannot marshal throws `FfiError { kind: "Schema" }`
//! naming the offender, rather than widening or ignoring it.

use std::{ffi::c_void, sync::Arc};

use libffi::middle::{Cif, Type, ffi_abi_FFI_DEFAULT_ABI};
use rquickjs::{Array, Ctx, FromJs, Object, Result, Value};

use crate::error::ErrorKind;

/// How deep a `{ struct }` may nest before den calls the schema malformed.
///
/// The schema is an ordinary JS object, so `const s = {}; s.struct = { a: s }`
/// is a cycle, and a recursive descent through one overflows the stack — which
/// aborts the process rather than throwing. Sixteen is past any C struct
/// anybody nests by hand, and `layout()` needs no grant, so this bound is what
/// keeps an ungranted module from taking the host down (§5.3).
const MAX_STRUCT_DEPTH: usize = 16;

/// One C value type: a scalar, `void`, or a struct passed by value.
///
/// Every scalar width is distinct even where two share a Rust type on this
/// target, because the CIF and the range check both read this.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum NativeType {
    I8,
    U8,
    I16,
    U16,
    I32,
    U32,
    I64,
    U64,
    Isize,
    Usize,
    F32,
    F64,
    Bool,
    Pointer,
    Void,
    /// `{ struct: { field: type, … } }`, laid out at `open()` (§4.2).
    Struct(Arc<StructLayout>),
}

impl NativeType {
    /// The declared type of one result, one static or one struct field.
    pub fn parse<'js>(ctx: &Ctx<'js>, symbol: &str, declared: &Value<'js>) -> Result<Self> {
        Self::nested(ctx, symbol, declared, 0)
    }

    /// [`Self::parse`], carrying how many `{ struct }` wrappers it is already
    /// inside — see [`MAX_STRUCT_DEPTH`].
    fn nested<'js>(
        ctx: &Ctx<'js>, symbol: &str, declared: &Value<'js>, depth: usize,
    ) -> Result<Self> {
        if let Some(name) = Self::type_name(declared)? {
            return Self::named(ctx, symbol, &name);
        }
        if let Some(object) = declared.as_object()
            && let Some(fields) = object.get::<_, Option<Object<'js>>>("struct")?
        {
            refuse_unknown_keys(ctx, symbol, object, &["struct"])?;
            if depth >= MAX_STRUCT_DEPTH {
                return Err(ErrorKind::Schema.throw_for(
                    ctx,
                    format_args!(
                        "symbol `{symbol}`: a struct type may not nest more than \
                         {MAX_STRUCT_DEPTH} deep — this one is either enormous or cyclic"
                    ),
                    symbol,
                ));
            }
            return Ok(Self::Struct(Arc::new(StructLayout::parse(
                ctx,
                symbol,
                &fields,
                depth + 1,
            )?)));
        }
        Err(ErrorKind::Schema.throw_for(
            ctx,
            format_args!(
                "symbol `{symbol}`: a type is a type name or `{{ struct: {{ … }} }}` — `{{ \
                 callback }}` is a parameter type only"
            ),
            symbol,
        ))
    }

    /// The name a type was declared by, when it was declared by one at all —
    /// which is what tells `buffer` apart before [`Self::named`] rejects it as
    /// a result type.
    fn type_name(declared: &Value<'_>) -> Result<Option<String>> {
        declared
            .as_string()
            .map(rquickjs::String::to_string)
            .transpose()
    }

    fn named(ctx: &Ctx<'_>, symbol: &str, name: &str) -> Result<Self> {
        match name {
            "i8" => Ok(Self::I8),
            "u8" => Ok(Self::U8),
            "i16" => Ok(Self::I16),
            "u16" => Ok(Self::U16),
            "i32" => Ok(Self::I32),
            "u32" => Ok(Self::U32),
            "i64" => Ok(Self::I64),
            "u64" => Ok(Self::U64),
            "isize" => Ok(Self::Isize),
            "usize" => Ok(Self::Usize),
            "f32" => Ok(Self::F32),
            "f64" => Ok(Self::F64),
            "bool" => Ok(Self::Bool),
            "pointer" => Ok(Self::Pointer),
            "void" => Ok(Self::Void),
            // A pointer C returns carries no length, so there is nothing to
            // build a `Uint8Array` from; a static has the same problem (§4.4).
            "buffer" => {
                Err(ErrorKind::Schema.throw_for(
                    ctx,
                    format_args!(
                        "symbol `{symbol}`: `buffer` is an argument type, not a result or static \
                         type"
                    ),
                    symbol,
                ))
            }
            other => {
                Err(ErrorKind::Schema.throw_for(
                    ctx,
                    format_args!("symbol `{symbol}`: `{other}` is not a type den:ffi knows"),
                    symbol,
                ))
            }
        }
    }

    /// The declared type of one parameter or one struct field. `void` is a
    /// result type only: a C function takes no `void` argument, and accepting
    /// one would make the CIF disagree with the argument array.
    fn parse_value<'js>(
        ctx: &Ctx<'js>, symbol: &str, declared: &Value<'js>, depth: usize,
    ) -> Result<Self> {
        match Self::nested(ctx, symbol, declared, depth)? {
            Self::Void => {
                Err(ErrorKind::Schema.throw_for(
                    ctx,
                    format_args!(
                        "symbol `{symbol}`: `void` is a result type, not a parameter or field type"
                    ),
                    symbol,
                ))
            }
            value => Ok(value),
        }
    }

    /// The name this type has in the schema, for error messages.
    pub const fn name(&self) -> &'static str {
        match self {
            Self::I8 => "i8",
            Self::U8 => "u8",
            Self::I16 => "i16",
            Self::U16 => "u16",
            Self::I32 => "i32",
            Self::U32 => "u32",
            Self::I64 => "i64",
            Self::U64 => "u64",
            Self::Isize => "isize",
            Self::Usize => "usize",
            Self::F32 => "f32",
            Self::F64 => "f64",
            Self::Bool => "bool",
            Self::Pointer => "pointer",
            Self::Void => "void",
            Self::Struct(_) => "struct",
        }
    }

    /// How many bytes one value of this type occupies, which is how much of a
    /// C cell may be copied out of it (see [`crate::marshal::Cell`]).
    pub fn size(&self) -> usize {
        match self {
            Self::I8 | Self::U8 | Self::Bool => size_of::<u8>(),
            Self::I16 | Self::U16 => size_of::<u16>(),
            Self::I32 | Self::U32 | Self::F32 => size_of::<u32>(),
            Self::I64 | Self::U64 | Self::F64 => size_of::<u64>(),
            Self::Isize | Self::Usize => size_of::<usize>(),
            Self::Pointer => size_of::<*mut c_void>(),
            Self::Void => 0,
            Self::Struct(layout) => layout.size,
        }
    }

    /// What this type must be aligned to inside a struct. It is the C
    /// alignment because it is Rust's for the same target — which is the
    /// assumption [`StructLayout::agrees_with_libffi`] exists to check.
    fn align(&self) -> usize {
        match self {
            Self::I8 | Self::U8 | Self::Bool | Self::Void => align_of::<u8>(),
            Self::I16 | Self::U16 => align_of::<u16>(),
            Self::I32 | Self::U32 | Self::F32 => align_of::<u32>(),
            Self::I64 | Self::U64 | Self::F64 => align_of::<u64>(),
            Self::Isize | Self::Usize => align_of::<usize>(),
            Self::Pointer => align_of::<*mut c_void>(),
            Self::Struct(layout) => layout.align,
        }
    }

    /// The libffi description this type calls for. C's `_Bool` is one byte, so
    /// it rides the `u8` class; the marshaller is what normalises 0/1.
    pub fn ffi_type(&self) -> Type {
        match self {
            Self::I8 => Type::i8(),
            Self::U8 | Self::Bool => Type::u8(),
            Self::I16 => Type::i16(),
            Self::U16 => Type::u16(),
            Self::I32 => Type::i32(),
            Self::U32 => Type::u32(),
            Self::I64 => Type::i64(),
            Self::U64 => Type::u64(),
            Self::Isize => Type::isize(),
            Self::Usize => Type::usize(),
            Self::F32 => Type::f32(),
            Self::F64 => Type::f64(),
            Self::Pointer => Type::pointer(),
            Self::Void => Type::void(),
            Self::Struct(layout) => layout.ffi_type(),
        }
    }
}

/// One field of a struct, and where den computed it to sit.
#[derive(Debug, Eq, PartialEq)]
pub struct StructField {
    pub name:     String,
    pub declared: NativeType,
    pub offset:   usize,
}

/// A struct passed or returned by value.
///
/// The offsets are den's own arithmetic — the only arithmetic in this crate —
/// so they are checked against libffi's before any call happens (§4.2). Named
/// fields are also what makes Deno's unchecked-struct hole unrepresentable:
/// there is no caller-supplied buffer here to be the wrong size.
#[derive(Debug, Eq, PartialEq)]
pub struct StructLayout {
    pub fields: Vec<StructField>,
    pub size:   usize,
    pub align:  usize,
}

impl StructLayout {
    /// `offset = align_up(cursor, align_of(field))`, `align = max(field
    /// aligns)`, `size = align_up(cursor, align)` — the C rule, and then
    /// libffi is asked whether it agrees.
    fn parse<'js>(
        ctx: &Ctx<'js>, symbol: &str, declared: &Object<'js>, depth: usize,
    ) -> Result<Self> {
        let (mut fields, mut cursor, mut align) = (Vec::new(), 0_usize, 1_usize);
        for entry in declared.props::<String, Value<'js>>() {
            let (name, declared) = entry?;
            let declared = NativeType::parse_value(ctx, symbol, &declared, depth)?;
            let offset = cursor.next_multiple_of(declared.align());
            cursor = offset + declared.size();
            align = align.max(declared.align());
            fields.push(StructField {
                name,
                declared,
                offset,
            });
        }
        if fields.is_empty() {
            return Err(ErrorKind::Schema.throw_for(
                ctx,
                format_args!("symbol `{symbol}`: a struct type needs at least one field"),
                symbol,
            ));
        }
        let computed = Self {
            fields,
            size: cursor.next_multiple_of(align),
            align,
        };
        computed.agrees_with_libffi(ctx, symbol)?;
        Ok(computed)
    }

    fn ffi_type(&self) -> Type {
        Type::structure(self.fields.iter().map(|field| field.declared.ffi_type()))
    }

    /// libffi lays the same fields out for the target ABI, and it is what will
    /// actually place them in registers — so a disagreement with den's
    /// arithmetic is `Layout` at `open()` rather than a silently wrong call on
    /// a target nobody has tested (§8 question 1).
    fn agrees_with_libffi(&self, ctx: &Ctx<'_>, symbol: &str) -> Result<()> {
        let mut described = self.ffi_type();
        let offsets = described
            .struct_offsets(ffi_abi_FFI_DEFAULT_ABI)
            .map_err(|error| {
                ErrorKind::Layout.throw_for(
                    ctx,
                    format_args!("symbol `{symbol}`: libffi refused this struct type: {error:?}"),
                    symbol,
                )
            })?;
        // SAFETY: `as_raw_ptr` hands back the `ffi_type` this `Type` owns, and
        // `struct_offsets` has just laid it out — which is what initialises
        // `size` and `alignment` — so both fields are readable and live for as
        // long as `described`.
        let (size, align) = unsafe {
            let described = &*described.as_raw_ptr();
            (described.size, usize::from(described.alignment))
        };
        let disagrees = size != self.size
            || align != self.align
            || offsets.len() != self.fields.len()
            || offsets
                .iter()
                .zip(&self.fields)
                .any(|(libffi, field)| *libffi != field.offset);
        if disagrees {
            return Err(ErrorKind::Layout.throw_for(
                ctx,
                format_args!(
                    "symbol `{symbol}`: den laid this struct out as size {} align {} offsets \
                     {:?}, libffi as size {size} align {align} offsets {offsets:?} — den's ABI \
                     arithmetic is wrong on this target",
                    self.size,
                    self.align,
                    self.fields
                        .iter()
                        .map(|field| field.offset)
                        .collect::<Vec<_>>()
                ),
                symbol,
            ));
        }
        Ok(())
    }

    /// `layout(type)` — what den believes about a struct, so that a script can
    /// assert it against the header it is binding. No computed layout can see
    /// `#pragma pack`, bitfields or `-fshort-enums`; publishing den's answer is
    /// the only way a caller can find out that den is wrong (§5.2).
    pub fn query<'js>(ctx: &Ctx<'js>, declared: &Value<'js>) -> Result<Object<'js>> {
        let NativeType::Struct(layout) = NativeType::parse(ctx, "layout", declared)? else {
            return Err(ErrorKind::Schema.throw(
                ctx,
                "`layout` describes a struct type — `layout({ struct: { … } })`",
            ));
        };
        let offsets = Object::new(ctx.clone())?;
        for field in &layout.fields {
            offsets.set(field.name.as_str(), field.offset)?;
        }
        let described = Object::new(ctx.clone())?;
        described.set("size", layout.size)?;
        described.set("align", layout.align)?;
        described.set("offsets", offsets)?;
        Ok(described)
    }
}

/// What a parameter may be.
///
/// `buffer` lives here and only here: it is an argument type, because a
/// pointer C hands back carries no length to build a `Uint8Array` from (§4.4).
/// Keeping it out of [`NativeType`] is what makes "not a result, not a static"
/// a property of the types rather than a check someone can forget.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum ParamType {
    /// A scalar or a struct: something C is handed by value.
    Value(NativeType),
    /// A `Uint8Array` whose bytes C borrows for exactly one call (§4.6).
    Buffer,
    /// `{ callback: { params, result } }` — a function pointer into JS. The
    /// signature is kept so that the handle passed at the call site can be
    /// checked against the slot it is passed to: a mismatch is a
    /// wrong-signature call into libffi, which is undefined behaviour with no
    /// diagnostic at any layer (§0 fact 1). The `.d.ts` brand makes the same
    /// check at compile time; this is the one that holds for plain JS.
    Callback(Arc<FnSig>),
}

impl ParamType {
    pub fn parse<'js>(ctx: &Ctx<'js>, symbol: &str, declared: &Value<'js>) -> Result<Self> {
        // A `{ callback }` wrapper is the only object a parameter may be, and
        // it is what tells a function-pointer slot apart from a plain
        // `pointer` — the slot's signature is what the handle is checked
        // against at the call site.
        if let Some(object) = declared.as_object()
            && let Some(signature) = object.get::<_, Option<Value<'js>>>("callback")?
        {
            refuse_unknown_keys(ctx, symbol, object, &["callback"])?;
            return Ok(Self::Callback(Arc::new(FnSig::parse(
                ctx, symbol, &signature,
            )?)));
        }
        if NativeType::type_name(declared)?.as_deref() == Some("buffer") {
            return Ok(Self::Buffer);
        }
        Ok(Self::Value(NativeType::parse_value(
            ctx, symbol, declared, 0,
        )?))
    }

    /// Both a buffer and a callback reach C as an address.
    pub fn ffi_type(&self) -> Type {
        match self {
            Self::Value(value) => value.ffi_type(),
            Self::Buffer | Self::Callback(_) => Type::pointer(),
        }
    }
}

/// The signature of a JS function C may call.
///
/// Plain data, so it is `Send + Sync`: the trampoline reads it from whatever
/// thread C calls on, and phase 5 carries it to a `spawn_blocking` worker.
#[derive(Debug, Eq, PartialEq)]
pub struct FnSig {
    pub params: Vec<NativeType>,
    pub result: NativeType,
}

impl FnSig {
    const KEYS: [&'static str; 2] = ["params", "result"];

    pub fn parse<'js>(ctx: &Ctx<'js>, symbol: &str, declared: &Value<'js>) -> Result<Self> {
        let declared = declared.as_object().ok_or_else(|| {
            ErrorKind::Schema.throw_for(
                ctx,
                format_args!("symbol `{symbol}`: a callback needs a `{{ params, result }}` object"),
                symbol,
            )
        })?;
        refuse_unknown_keys(ctx, symbol, declared, &Self::KEYS)?;

        let params = read::<Array<'js>>(ctx, symbol, declared, "params", "an array of type names")?
            .ok_or_else(|| {
                ErrorKind::Schema.throw_for(
                    ctx,
                    format_args!("symbol `{symbol}`: a callback's `params` must be an array"),
                    symbol,
                )
            })?
            .iter::<Value<'js>>()
            .map(|declared| {
                let declared = declared?;
                // A buffer is a length den is told, and C tells it nothing:
                // the trampoline is handed a bare address, so there is no
                // length to build a `Uint8Array` from.
                if NativeType::type_name(&declared)?.as_deref() == Some("buffer") {
                    return Err(ErrorKind::Schema.throw_for(
                        ctx,
                        format_args!(
                            "symbol `{symbol}`: a callback parameter cannot be `buffer` — C hands \
                             den an address with no length; take a `pointer` and use `ptr.view`"
                        ),
                        symbol,
                    ));
                }
                let parsed = NativeType::parse_value(ctx, symbol, &declared, 0)?;
                Self::refuse_struct(ctx, symbol, &parsed, "parameter")?;
                Ok(parsed)
            })
            .collect::<Result<Vec<_>>>()?;

        let result = declared
            .get::<_, Option<Value<'js>>>("result")?
            .ok_or_else(|| {
                ErrorKind::Schema.throw_for(
                    ctx,
                    format_args!("symbol `{symbol}`: a callback's `result` must be a type name"),
                    symbol,
                )
            })
            .and_then(|declared| NativeType::parse(ctx, symbol, &declared))?;
        Self::refuse_struct(ctx, symbol, &result, "result")?;

        Ok(Self { params, result })
    }

    /// A callback's arguments are copied out of libffi's argument vector as
    /// fixed-width [`crate::marshal::Cell`]s and its answer is written back
    /// through one, so a struct by value has nowhere to go in either
    /// direction. Refused by name rather than truncated — and the published
    /// `FnSig` in `types/den-ffi.d.ts` excludes it too, so this is the
    /// backstop for plain JS.
    fn refuse_struct(
        ctx: &Ctx<'_>, symbol: &str, declared: &NativeType, position: &str,
    ) -> Result<()> {
        if matches!(declared, NativeType::Struct(_)) {
            return Err(ErrorKind::Schema.throw_for(
                ctx,
                format_args!(
                    "symbol `{symbol}`: a callback {position} cannot be a struct by value — den \
                     carries a callback's values as register-wide cells"
                ),
                symbol,
            ));
        }
        Ok(())
    }

    /// The C signature of a bound symbol: a `buffer` and a `{ callback }` both
    /// reach C as an address, so both are `pointer` here. This is the plain,
    /// `Send` description a `nonblocking` call carries to its worker thread,
    /// where the CIF is rebuilt from it.
    pub fn of(params: &[ParamType], result: NativeType) -> Self {
        Self {
            params: params
                .iter()
                .map(|param| {
                    match param {
                        ParamType::Value(value) => value.clone(),
                        ParamType::Buffer | ParamType::Callback(_) => NativeType::Pointer,
                    }
                })
                .collect(),
            result,
        }
    }

    /// The CIF this signature describes. Cheap enough to rebuild per
    /// `nonblocking` call, which is what keeps the schema `Send`.
    pub fn cif(&self) -> Cif {
        Cif::new(
            self.params.iter().map(NativeType::ffi_type),
            self.result.ffi_type(),
        )
    }

    /// How this signature reads in an error message: `(i32, i32) => i32`.
    pub fn describe(&self) -> String {
        let params: Vec<&str> = self.params.iter().map(NativeType::name).collect();
        format!("({}) => {}", params.join(", "), self.result.name())
    }
}

/// How a bound symbol runs, which is also what decides whether a `Callback`
/// may be handed to it (§4.7).
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum CallMode {
    /// On the realm thread, holding the runtime lock for the whole call.
    Blocking,
    /// `nonblocking: true` — on a `spawn_blocking` worker, returning a Promise.
    Nonblocking,
}

/// One schema key, read as the shape it must have.
///
/// The engine's own conversion failure is an untyped `TypeError`, and every
/// refusal out of den:ffi carries a `kind` (§4.8) — so a key of the wrong
/// shape is `Schema` naming the key, exactly like a key den does not know.
fn read<'js, T: FromJs<'js>>(
    ctx: &Ctx<'js>, key: &str, declared: &Object<'js>, name: &str, expected: &str,
) -> Result<Option<T>> {
    let Some(value) = declared.get::<_, Option<Value<'js>>>(name)? else {
        return Ok(None);
    };
    T::from_js(ctx, value).map(Some).map_err(|_wrong_shape| {
        ErrorKind::Schema.throw_for(
            ctx,
            format_args!("symbol `{key}`: `{name}` must be {expected}"),
            key,
        )
    })
}

/// An ignored key is how Bun turns a `nonblocking` request into a synchronous
/// call with no diagnostic; den refuses instead.
fn refuse_unknown_keys(
    ctx: &Ctx<'_>, key: &str, declared: &Object<'_>, allowed: &[&str],
) -> Result<()> {
    for name in declared.keys::<String>() {
        let name = name?;
        if !allowed.contains(&name.as_str()) {
            return Err(ErrorKind::Schema.throw_for(
                ctx,
                format_args!(
                    "symbol `{key}`: key `{name}` is not supported yet — den:ffi currently reads \
                     {}",
                    allowed.join(", ")
                ),
                key,
            ));
        }
    }
    Ok(())
}

/// What a schema entry declares the symbol to be.
#[derive(Debug)]
pub enum SymbolKind {
    /// `{ params, result }` — a function to bind.
    Function {
        params: Vec<ParamType>,
        result: NativeType,
        mode:   CallMode,
    },
    /// `{ type }` — an exported variable, read once at `open()`.
    Static(NativeType),
}

/// One entry of the schema object, resolved.
#[derive(Debug)]
pub struct SymbolSpec {
    /// The name to `dlsym`, which `name` overrides independently of the key
    /// the symbol is published under.
    pub symbol:   String,
    /// A missing `optional` symbol is `null`; a missing required one fails
    /// `open()`.
    pub optional: bool,
    pub kind:     SymbolKind,
}

impl SymbolSpec {
    /// A `type` key is what makes an entry a static; the two key sets are
    /// disjoint apart from `name` and `optional`, so an entry carrying both is
    /// refused by whichever set it does not belong to.
    const FUNCTION_KEYS: [&'static str; 5] =
        ["params", "result", "name", "optional", "nonblocking"];
    const STATIC_KEYS: [&'static str; 3] = ["type", "name", "optional"];

    pub fn parse<'js>(ctx: &Ctx<'js>, key: &str, declared: &Value<'js>) -> Result<Self> {
        let declared = declared.as_object().ok_or_else(|| {
            ErrorKind::Schema.throw_for(
                ctx,
                format_args!(
                    "symbol `{key}`: expected a `{{ params, result }}` or `{{ type }}` object"
                ),
                key,
            )
        })?;

        let static_type = declared.get::<_, Option<Value<'js>>>("type")?;
        refuse_unknown_keys(
            ctx,
            key,
            declared,
            if static_type.is_some() {
                &Self::STATIC_KEYS
            } else {
                &Self::FUNCTION_KEYS
            },
        )?;

        let kind = match static_type {
            Some(declared_type) => {
                match NativeType::parse(ctx, key, &declared_type)? {
                    // A `void` variable has no bytes to read.
                    NativeType::Void => {
                        return Err(ErrorKind::Schema.throw_for(
                            ctx,
                            format_args!("symbol `{key}`: `void` is not a static symbol type"),
                            key,
                        ));
                    }
                    parsed => SymbolKind::Static(parsed),
                }
            }
            None => Self::parse_function(ctx, key, declared)?,
        };

        Ok(Self {
            symbol: read::<String>(ctx, key, declared, "name", "a string")?
                .unwrap_or_else(|| key.to_owned()),
            optional: read::<bool>(ctx, key, declared, "optional", "a boolean")?.unwrap_or(false),
            kind,
        })
    }

    fn parse_function<'js>(
        ctx: &Ctx<'js>, key: &str, declared: &Object<'js>,
    ) -> Result<SymbolKind> {
        let params = read::<Array<'js>>(ctx, key, declared, "params", "an array of type names")?
            .ok_or_else(|| {
                ErrorKind::Schema.throw_for(
                    ctx,
                    format_args!("symbol `{key}`: `params` must be an array of type names"),
                    key,
                )
            })?
            .iter::<Value<'js>>()
            .map(|declared| ParamType::parse(ctx, key, &declared?))
            .collect::<Result<Vec<_>>>()?;

        let result = declared
            .get::<_, Option<Value<'js>>>("result")?
            .ok_or_else(|| {
                ErrorKind::Schema.throw_for(
                    ctx,
                    format_args!("symbol `{key}`: `result` must be a type name"),
                    key,
                )
            })
            .and_then(|declared| NativeType::parse(ctx, key, &declared))?;

        let mode = if read::<bool>(ctx, key, declared, "nonblocking", "a boolean")?.unwrap_or(false)
        {
            CallMode::Nonblocking
        } else {
            CallMode::Blocking
        };
        // §4.6: a `buffer` lends the view's own bytes for exactly the length of
        // the call. A `nonblocking` call outlives its call site, and JS is free
        // to detach or transfer that buffer while the worker is inside C, so
        // the two are refused together rather than lent across a thread.
        if mode == CallMode::Nonblocking && params.contains(&ParamType::Buffer) {
            return Err(ErrorKind::Schema.throw_for(
                ctx,
                format_args!(
                    "symbol `{key}`: a `buffer` argument cannot be `nonblocking` — its bytes are \
                     borrowed for one call, and JS keeps running during this one"
                ),
                key,
            ));
        }

        Ok(SymbolKind::Function {
            params,
            result,
            mode,
        })
    }
}
