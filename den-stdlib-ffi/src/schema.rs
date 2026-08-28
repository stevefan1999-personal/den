//! The schema — plain data, read once at `open()`.
//!
//! `docs/research/19-den-ffi.md` §3.1: the same value types the call site in
//! `types/den-ffi.d.ts`, drives the CIF here, and is validated at the
//! boundary. Phase 1 understands the `i32` / `f64` / `void` corner of it and
//! throws `FfiError { kind: "Schema" }` — naming the offender — for
//! everything else, rather than widening or ignoring it.

use libffi::middle::Type;
use rquickjs::{Array, Ctx, Object, Result, Value};

use crate::error::ErrorKind;

/// What phase 1 can marshal.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum NativeType {
    I32,
    F64,
    Void,
}

impl NativeType {
    /// The declared type of one parameter or one result.
    fn parse(ctx: &Ctx<'_>, symbol: &str, declared: &Value<'_>) -> Result<Self> {
        match declared.as_string().map(rquickjs::String::to_string) {
            Some(Ok(name)) => {
                match name.as_str() {
                    "i32" => Ok(Self::I32),
                    "f64" => Ok(Self::F64),
                    "void" => Ok(Self::Void),
                    other => {
                        Err(ErrorKind::Schema.throw_for(
                            ctx,
                            format_args!(
                                "symbol `{symbol}`: type `{other}` is not supported yet — den:ffi \
                                 currently marshals `i32`, `f64` and `void`"
                            ),
                            symbol,
                        ))
                    }
                }
            }
            Some(Err(error)) => Err(error),
            None => {
                Err(ErrorKind::Schema.throw_for(
                    ctx,
                    format_args!(
                        "symbol `{symbol}`: a type must be one of the names `i32`, `f64` or \
                         `void` — struct and callback types are not supported yet"
                    ),
                    symbol,
                ))
            }
        }
    }

    /// The name this type has in the schema.
    pub const fn name(self) -> &'static str {
        match self {
            Self::I32 => "i32",
            Self::F64 => "f64",
            Self::Void => "void",
        }
    }

    /// The libffi description this type calls for.
    pub fn ffi_type(self) -> Type {
        match self {
            Self::I32 => Type::i32(),
            Self::F64 => Type::f64(),
            Self::Void => Type::void(),
        }
    }
}

/// One entry of the schema object: the signature of one exported function.
#[derive(Debug)]
pub struct FnSpec {
    pub params: Vec<NativeType>,
    pub result: NativeType,
}

impl FnSpec {
    /// The keys phase 1 reads. `nonblocking`, `optional` and the static-symbol
    /// `type` are part of the published schema and are refused here by name,
    /// so that a script never silently gets a *synchronous* call it asked to
    /// have run off-thread.
    const KEYS: [&'static str; 2] = ["params", "result"];

    pub fn parse<'js>(ctx: &Ctx<'js>, symbol: &str, declared: &Value<'js>) -> Result<Self> {
        let declared = declared.as_object().ok_or_else(|| {
            ErrorKind::Schema.throw_for(
                ctx,
                format_args!("symbol `{symbol}`: expected a `{{ params, result }}` object"),
                symbol,
            )
        })?;
        Self::refuse_unknown_keys(ctx, symbol, declared)?;

        let params = declared
            .get::<_, Option<Array<'js>>>("params")?
            .ok_or_else(|| {
                ErrorKind::Schema.throw_for(
                    ctx,
                    format_args!("symbol `{symbol}`: `params` must be an array of type names"),
                    symbol,
                )
            })?
            .iter::<Value<'js>>()
            .map(|declared| Self::parse_param(ctx, symbol, declared?))
            .collect::<Result<Vec<_>>>()?;

        let result = declared
            .get::<_, Option<Value<'js>>>("result")?
            .ok_or_else(|| {
                ErrorKind::Schema.throw_for(
                    ctx,
                    format_args!("symbol `{symbol}`: `result` must be a type name"),
                    symbol,
                )
            })
            .and_then(|declared| NativeType::parse(ctx, symbol, &declared))?;

        Ok(Self { params, result })
    }

    /// `void` is a result type only: a C function takes no `void` argument, and
    /// accepting one would make the CIF disagree with the argument array.
    fn parse_param(ctx: &Ctx<'_>, symbol: &str, declared: Value<'_>) -> Result<NativeType> {
        match NativeType::parse(ctx, symbol, &declared)? {
            NativeType::Void => {
                Err(ErrorKind::Schema.throw_for(
                    ctx,
                    format_args!(
                        "symbol `{symbol}`: `void` is a result type, not a parameter type"
                    ),
                    symbol,
                ))
            }
            parsed => Ok(parsed),
        }
    }

    /// An ignored key is how Bun turns a `nonblocking` request into a
    /// synchronous call with no diagnostic; den refuses instead.
    fn refuse_unknown_keys(ctx: &Ctx<'_>, symbol: &str, declared: &Object<'_>) -> Result<()> {
        for key in declared.keys::<String>() {
            let key = key?;
            if !Self::KEYS.contains(&key.as_str()) {
                return Err(ErrorKind::Schema.throw_for(
                    ctx,
                    format_args!(
                        "symbol `{symbol}`: key `{key}` is not supported yet — den:ffi currently \
                         reads only `params` and `result`"
                    ),
                    symbol,
                ));
            }
        }
        Ok(())
    }
}
