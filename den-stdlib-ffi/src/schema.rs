//! The schema — plain data, read once at `open()`.
//!
//! `docs/research/19-den-ffi.md` §3.1: the same value types the call site in
//! `types/den-ffi.d.ts`, drives the CIF here, and is validated at the
//! boundary. Everything the current phases cannot marshal — structs,
//! callbacks, `nonblocking` — throws `FfiError { kind: "Schema" }` naming the
//! offender, rather than widening or ignoring it.

use std::sync::Arc;

use libffi::middle::Type;
use rquickjs::{Array, Ctx, Object, Result, Value};

use crate::error::ErrorKind;

/// One C scalar. Every width is distinct even where two share a Rust type on
/// this target, because the CIF and the range check both read this.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
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
}

impl NativeType {
    /// The declared type of one result or one static.
    fn parse(ctx: &Ctx<'_>, symbol: &str, declared: &Value<'_>) -> Result<Self> {
        Self::named(ctx, symbol, &Self::type_name(ctx, symbol, declared)?)
    }

    /// A type is named by a string. An object is a struct or a callback type,
    /// neither of which den can marshal yet, and anything else is malformed.
    fn type_name(ctx: &Ctx<'_>, symbol: &str, declared: &Value<'_>) -> Result<String> {
        match declared.as_string().map(rquickjs::String::to_string) {
            Some(Ok(name)) => Ok(name),
            Some(Err(error)) => Err(error),
            None => {
                Err(ErrorKind::Schema.throw_for(
                    ctx,
                    format_args!(
                        "symbol `{symbol}`: a type must be a type name — struct types are not \
                         supported yet, and `{{ callback }}` is a parameter type only"
                    ),
                    symbol,
                ))
            }
        }
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
                    format_args!(
                        "symbol `{symbol}`: type `{other}` is not supported yet — den:ffi \
                         currently marshals the scalar types and `void`"
                    ),
                    symbol,
                ))
            }
        }
    }

    /// The declared type of one parameter. `void` is a result type only: a C
    /// function takes no `void` argument, and accepting one would make the CIF
    /// disagree with the argument array.
    fn parse_param(ctx: &Ctx<'_>, symbol: &str, name: &str) -> Result<Self> {
        match Self::named(ctx, symbol, name)? {
            Self::Void => {
                Err(ErrorKind::Schema.throw_for(
                    ctx,
                    format_args!(
                        "symbol `{symbol}`: `void` is a result type, not a parameter type"
                    ),
                    symbol,
                ))
            }
            scalar => Ok(scalar),
        }
    }

    /// The name this type has in the schema, for error messages.
    pub const fn name(self) -> &'static str {
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
        }
    }

    /// The libffi description this type calls for. C's `_Bool` is one byte, so
    /// it rides the `u8` class; the marshaller is what normalises 0/1.
    pub fn ffi_type(self) -> Type {
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
        }
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
    Scalar(NativeType),
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
        let name = NativeType::type_name(ctx, symbol, declared)?;
        if name == "buffer" {
            return Ok(Self::Buffer);
        }
        Ok(Self::Scalar(NativeType::parse_param(ctx, symbol, &name)?))
    }

    /// Both a buffer and a callback reach C as an address.
    pub fn ffi_type(&self) -> Type {
        match self {
            Self::Scalar(scalar) => scalar.ffi_type(),
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

        let params = declared
            .get::<_, Option<Array<'js>>>("params")?
            .ok_or_else(|| {
                ErrorKind::Schema.throw_for(
                    ctx,
                    format_args!("symbol `{symbol}`: a callback's `params` must be an array"),
                    symbol,
                )
            })?
            .iter::<Value<'js>>()
            .map(|declared| {
                let name = NativeType::type_name(ctx, symbol, &declared?)?;
                // A buffer is a length den is told, and C tells it nothing:
                // the trampoline is handed a bare address, so there is no
                // length to build a `Uint8Array` from.
                if name == "buffer" {
                    return Err(ErrorKind::Schema.throw_for(
                        ctx,
                        format_args!(
                            "symbol `{symbol}`: a callback parameter cannot be `buffer` — C hands \
                             den an address with no length; take a `pointer` and use `ptr.view`"
                        ),
                        symbol,
                    ));
                }
                NativeType::parse_param(ctx, symbol, &name)
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

        Ok(Self { params, result })
    }

    /// How this signature reads in an error message: `(i32, i32) => i32`.
    pub fn describe(&self) -> String {
        let params: Vec<&str> = self.params.iter().map(|param| param.name()).collect();
        format!("({}) => {}", params.join(", "), self.result.name())
    }
}

/// An ignored key is how Bun turns a `nonblocking` request into a synchronous
/// call with no diagnostic; den refuses instead. `nonblocking` is one of those
/// keys today, which is why nothing here has to say that a `buffer` parameter
/// may not be nonblocking — when phase 5 makes the key legal, that pairing has
/// to become its own refusal (§4.6), as does passing a `{ callback }` to a
/// symbol that is not `nonblocking` (§4.7).
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
    const FUNCTION_KEYS: [&'static str; 4] = ["params", "result", "name", "optional"];
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
            symbol: declared
                .get::<_, Option<String>>("name")?
                .unwrap_or_else(|| key.to_owned()),
            optional: declared
                .get::<_, Option<bool>>("optional")?
                .unwrap_or(false),
            kind,
        })
    }

    fn parse_function<'js>(
        ctx: &Ctx<'js>, key: &str, declared: &Object<'js>,
    ) -> Result<SymbolKind> {
        let params = declared
            .get::<_, Option<Array<'js>>>("params")?
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

        Ok(SymbolKind::Function { params, result })
    }
}
