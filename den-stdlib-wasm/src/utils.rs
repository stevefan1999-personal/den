//! JS ⟷ wasm value coercion, per the JS API's `ToJSValue` /
//! `ToWebAssemblyValue` / `DefaultValue` algorithms (§ "JavaScript to
//! WebAssembly value conversion").
//!
//! The conversion *into* wasm is type-directed: the target [`ValType`] decides
//! the algorithm, not the shape of the incoming JS value. That is the whole
//! point — `1` is a valid `f32`, a valid `f64` and a valid `i32` argument, but
//! not a valid `i64` one.

use std::cell::RefCell;

use rquickjs::{
    Array, BigInt, Coerced, Ctx, Exception, FromJs, Function, IntoJs, JsLifetime, Result, Symbol,
    Value, function::Rest,
};
use wasmtime::{Func, RefType, Val, ValType};

use crate::{backend, error::throw_runtime_error, memory::MemoryBuffers, store::Store};

/// A wasm value together with the coercions that move it across the JS
/// boundary.
#[derive(Clone, Debug)]
pub struct WasmValue(pub Val);

impl WasmValue {
    /// `DefaultValue(valuetype)` — the zero of a type, used when a
    /// `Global`/`Table` descriptor omits its initial value.
    pub fn default_for(ty: &ValType) -> core::result::Result<Self, &'static str> {
        Val::default_for_ty(ty)
            .filter(|_| !matches!(ty, ValType::V128))
            .map(Self)
            .ok_or("this WebAssembly type has no default value")
    }

    /// `ToWebAssemblyValue(v, type)` — JS to wasm, directed by the target type.
    pub fn from_js<'js>(ctx: &Ctx<'js>, value: &Value<'js>, ty: &ValType) -> Result<Self> {
        let value = match ty {
            ValType::I32 => Val::I32(Coerced::<i32>::from_js(ctx, value.clone())?.0),
            ValType::I64 => Val::I64(Self::to_big_int_64(ctx, value)?),
            ValType::F32 => Val::from(Coerced::<f64>::from_js(ctx, value.clone())?.0 as f32),
            ValType::F64 => Val::from(Coerced::<f64>::from_js(ctx, value.clone())?.0),
            ValType::V128 => {
                return Err(Exception::throw_type(
                    ctx,
                    "v128 values cannot cross the JS boundary",
                ));
            }
            ValType::Ref(reference)
                if value.is_null()
                    && (reference.matches(&RefType::FUNCREF)
                        || reference.matches(&RefType::EXTERNREF)
                        || reference.matches(&RefType::ANYREF)) =>
            {
                Self::default_for(ty)
                    .map_err(|err| Exception::throw_type(ctx, err))?
                    .0
            }
            // Any JS value at all is a valid `externref`; a `funcref` needs a
            // `[[FunctionAddress]]`, which only an Exported Function has.
            ValType::Ref(reference) if reference.matches(&RefType::EXTERNREF) => {
                HostReferences::extern_ref(ctx, value)?
            }
            ValType::Ref(reference) if reference.matches(&RefType::FUNCREF) => {
                HostReferences::func_ref(ctx, value)?
            }
            // `anyref` is not in the spec's `ValueType` enum at all — it can only
            // reach here from a function signature that uses the GC
            // proposal, and den has no `i31`/`struct`/`array` conversions.
            ValType::Ref(reference) if reference.matches(&RefType::ANYREF) => {
                return Err(Exception::throw_type(
                    ctx,
                    "only null is convertible to a WebAssembly anyref",
                ));
            }
            ValType::Ref(_) => {
                return Err(Exception::throw_type(
                    ctx,
                    "this WebAssembly type cannot cross the JS boundary",
                ));
            }
        };
        Ok(Self(value))
    }

    /// `ToJSValue(w)` — wasm to JS. `i64` becomes a `BigInt`, floats become
    /// real `Number`s.
    pub fn to_js<'js>(&self, ctx: &Ctx<'js>) -> Result<Value<'js>> {
        match &self.0 {
            Val::I32(x) => x.into_js(ctx),
            Val::I64(x) => BigInt::from_i64(ctx.clone(), *x)?.into_js(ctx),
            Val::F32(bits) => f64::from(f32::from_bits(*bits)).into_js(ctx),
            Val::F64(bits) => f64::from_bits(*bits).into_js(ctx),
            Val::V128(_) => {
                Err(Exception::throw_type(
                    ctx,
                    "v128 values cannot cross the JS boundary",
                ))
            }
            Val::FuncRef(None)
            | Val::ExternRef(None)
            | Val::AnyRef(None)
            | Val::ExnRef(None)
            | Val::ContRef(None) => Ok(Value::new_null(ctx.clone())),
            // The store borrow ends before either branch runs: building an
            // Exported Function reads the signature again, and a host value is
            // fetched from the JS-side registry.
            Val::FuncRef(Some(_))
            | Val::ExternRef(Some(_))
            | Val::AnyRef(Some(_))
            | Val::ExnRef(Some(_))
            | Val::ContRef(Some(_)) => {
                let store = Store::from_ctx(ctx)?;
                let reference =
                    store.with_mut(ctx, |store| Reference::read(ctx, store, &self.0))?;
                match reference {
                    // No module index is reachable from a bare reference value,
                    // so a funcref first seen here gets the anonymous callable
                    // — `instance.exports` names the ones it creates.
                    Reference::Func(func) => {
                        HostReferences::exported_function(ctx, func, None).map(Function::into_value)
                    }
                    Reference::Host(index) => HostReferences::value(ctx, index),
                    Reference::Foreign => {
                        Err(Exception::throw_type(
                            ctx,
                            "this WebAssembly reference is not representable in JS",
                        ))
                    }
                }
            }
        }
    }

    pub fn into_inner(self) -> Val { self.0 }

    /// `ToBigInt64(v)`: a Number is a `TypeError` (that is the whole reason
    /// `i64` needs its own path), everything else goes through JS's own
    /// `ToBigInt` and is then wrapped modulo 2^64 — `2n ** 63n` is `i64::MIN`,
    /// not a conversion failure.
    fn to_big_int_64<'js>(ctx: &Ctx<'js>, value: &Value<'js>) -> Result<i64> {
        if value.is_number() {
            return Err(Exception::throw_type(
                ctx,
                "cannot convert a Number to a WebAssembly i64 value; pass a BigInt",
            ));
        }
        let as_int_n = ctx
            .globals()
            .get::<_, rquickjs::Object>("BigInt")?
            .get::<_, Function>("asIntN")?;
        as_int_n.call::<_, BigInt>((64, value.clone()))?.to_i64()
    }
}

/// WebIDL `[EnforceRange] unsigned long`, the declared type of every numeric
/// argument and dictionary member in the WebAssembly IDL — descriptor
/// `initial`/`maximum`, `Memory.grow`'s and `Table.grow`'s delta, `Table`'s
/// index and `Exception.getArg`'s.
///
/// `Coerced<u64>` is a *different* algorithm and gets the observable errors
/// wrong: it reads `NaN` as `0`, wraps negatives around and saturates the
/// infinities, where WebIDL rejects all three with a `TypeError` raised before
/// the operation's own steps run.
#[derive(Clone, Copy, Debug)]
pub struct EnforceRange(pub u32);

impl<'js> FromJs<'js> for EnforceRange {
    fn from_js(ctx: &Ctx<'js>, value: Value<'js>) -> Result<Self> {
        // ToNumber first — that is the step which runs a `valueOf` hook — then
        // WebIDL's integer conversion: reject the non-finite values, truncate
        // toward zero, and reject anything outside the target type.
        let number = Coerced::<f64>::from_js(ctx, value)?.0;
        let truncated = number.trunc();
        if !number.is_finite() || truncated < 0.0 || truncated > f64::from(u32::MAX) {
            return Err(Exception::throw_type(
                ctx,
                &format!("{number} is out of range for an unsigned long"),
            ));
        }
        Ok(Self(truncated as u32))
    }
}

impl EnforceRange {
    /// The value as the `u64` wasmtime counts sizes and indices in.
    pub fn size(self) -> u64 { u64::from(self.0) }
}

/// The reference a [`Val`] carries in a shape the JS-side cache can use.
enum Reference {
    /// A function reference. Its JS side is an Exported Function.
    Func(Func),
    /// An `externref` carrying the index of a JS value in [`HostReferences`].
    Host(usize),
    /// A reference this layer cannot name: a GC reference, or an `externref`
    /// whose payload some other embedder allocated.
    Foreign,
}

impl Reference {
    fn read(ctx: &Ctx<'_>, store: &mut backend::Store, value: &Val) -> Result<Self> {
        Ok(match value {
            Val::FuncRef(Some(func)) => Self::Func(*func),
            Val::ExternRef(Some(reference)) => {
                reference
                    .data(&*store)
                    .map_err(|err| throw_runtime_error(ctx, err))?
                    .and_then(|data| data.downcast_ref::<usize>().copied())
                    .map_or(Self::Foreign, Self::Host)
            }
            _ => Self::Foreign,
        })
    }

    /// Allocate an `externref` around `index`.
    ///
    /// The `Rooted` handle is rooted in the store's outermost scope — den
    /// opens no `RootScope` — so it stays alive exactly as long as the store
    /// and the JS value it stands for.
    fn host(ctx: &Ctx<'_>, store: &mut backend::Store, index: usize) -> Result<Val> {
        ::wasmtime::ExternRef::new(store, index)
            .map(|reference| Val::ExternRef(Some(reference)))
            .map_err(|err| throw_runtime_error(ctx, err))
    }

    fn function(func: Func) -> Val { Val::FuncRef(Some(func)) }

    /// Identity of a `Func`, the key of the Exported Function cache.
    /// `to_raw` is the `VMFuncRef` pointer — wasmtime's own notion of which
    /// function this is within a store.
    fn identity(store: &mut backend::Store, func: &Func) -> String {
        format!("{:p}", func.to_raw(store))
    }
}

/// The JS side of every wasm reference value den has handed out.
///
/// Two caches, both owned by the JS context rather than by a wrapper object,
/// because a reference outlives the `Table` or `Global` it was read from:
///
/// * `values` — the JS values an `externref` stands for. wasmtime cannot hold a
///   `Value<'js>` itself (`ExternRef::new` wants `'static + Send + Sync`), so
///   the reference carries an index into this list while the value stays on the
///   JS side. That is what makes `table.set(0, o); table.get(0) === o` hold.
/// * `functions` — the spec's Exported Function cache (§ "create a new Exported
///   Function from funcaddr"), so that reading the same `funcref` twice yields
///   the same JS function object, and so that a function handed *back* to wasm
///   can be recognised as one den created.
///
/// ponytail: neither list ever shrinks and both are scanned linearly, so a
/// reference lives as long as the context and a context with thousands of
/// exported functions pays for it on every `Table.get`. The store already
/// keeps every instance alive for exactly as long, and a map keyed by the
/// engine's own identity is the upgrade if either ever shows up in a profile.
#[derive(JsLifetime, Default)]
pub struct HostReferences<'js> {
    values:    RefCell<Vec<Value<'js>>>,
    functions: RefCell<Vec<ExportedFunction<'js>>>,
}

/// One entry of the Exported Function cache.
#[derive(JsLifetime)]
struct ExportedFunction<'js> {
    identity: String,
    func:     Func,
    object:   Function<'js>,
}

impl<'js> HostReferences<'js> {
    /// `ToWebAssemblyValue(v, externref)`: every JS value is a valid
    /// `externref`, so this only has to make the value findable again.
    fn extern_ref(ctx: &Ctx<'js>, value: &Value<'js>) -> Result<Val> {
        let index = Self::with(ctx, |registry| {
            let mut values = registry
                .values
                .try_borrow_mut()
                .map_err(|_| Self::busy(ctx))?;
            values.push(value.clone());
            Ok(values.len() - 1)
        })?;
        Store::from_ctx(ctx)?.with_mut(ctx, |store| Reference::host(ctx, store, index))
    }

    /// The JS value an `externref` built by [`Self::extern_ref`] stands for.
    fn value(ctx: &Ctx<'js>, index: usize) -> Result<Value<'js>> {
        Self::with(ctx, |registry| {
            registry
                .values
                .try_borrow()
                .map_err(|_| Self::busy(ctx))?
                .get(index)
                .cloned()
                .ok_or_else(|| {
                    Exception::throw_internal(ctx, "a WebAssembly externref went missing")
                })
        })
    }

    /// The `[[FunctionAddress]]` of an Exported Function den created, if
    /// `value` is one. Used when an import should reuse that address rather
    /// than wrap the callable as a fresh host function — which is what
    /// makes `instance.exports.f === imported.f` hold.
    pub fn existing_func(ctx: &Ctx<'js>, value: &Value<'js>) -> Result<Option<Func>> {
        let address = value.as_object().and_then(|object| {
            object
                .get::<_, String>(Self::function_address_key(ctx).ok()?)
                .ok()
        });
        Self::with(ctx, |registry| {
            Ok(registry
                .functions
                .try_borrow()
                .map_err(|_| Self::busy(ctx))?
                .iter()
                .find(|entry| {
                    address.as_ref().is_some_and(|id| *id == entry.identity)
                        || *entry.object.as_value() == *value
                })
                .map(|entry| entry.func))
        })
    }

    /// A `Symbol.for` key so an Exported Function can be recognised after
    /// passing through JS, even when `Value` identity comparison misses.
    fn function_address_key(ctx: &Ctx<'js>) -> Result<Value<'js>> {
        Symbol::new_global(ctx.clone(), "den.WebAssembly.[[FunctionAddress]]")
            .map(Symbol::into_value)
    }

    /// `ToWebAssemblyValue(v, funcref)`: a `funcref` is a function *address*,
    /// and only an Exported Function has one. A plain JS function is a
    /// `TypeError` — there is no address den could invent for it.
    fn func_ref(ctx: &Ctx<'js>, value: &Value<'js>) -> Result<Val> {
        value.as_function().ok_or_else(|| {
            Exception::throw_type(
                ctx,
                "only null or a WebAssembly Exported Function is convertible to a funcref",
            )
        })?;
        Self::existing_func(ctx, value)?
            .map(Reference::function)
            .ok_or_else(|| {
                Exception::throw_type(
                    ctx,
                    "this function is not a WebAssembly Exported Function, so it has no function \
                     address to store in a funcref",
                )
            })
    }

    /// "Create a new Exported Function from funcaddr" — the *only* place a
    /// `wasmtime::Func` becomes a JS function, `instance.exports` included. The
    /// cache lookup is the spec's own first step, and is what makes both
    /// `table.get(0) === table.get(0)` and `table.get(0) ===
    /// instance.exports.f` hold.
    ///
    /// `name` is the function's index in its module, which the spec gives the
    /// callable as its `name`. Only the instantiation path knows it — a
    /// `funcref` read out of a table carries no index — and it is applied only
    /// on a cache miss, because renaming a callable JS already holds would
    /// change an object the spec says is created once.
    pub fn exported_function(
        ctx: &Ctx<'js>, func: Func, name: Option<u32>,
    ) -> Result<Function<'js>> {
        let store = Store::from_ctx(ctx)?;
        let (identity, arity) = store.with_mut(ctx, |backend_store| {
            Ok((
                Reference::identity(backend_store, &func),
                func.ty(&*backend_store).params().len(),
            ))
        })?;
        let cached = Self::with(ctx, |registry| {
            Ok(registry
                .functions
                .try_borrow()
                .map_err(|_| Self::busy(ctx))?
                .iter()
                .find(|entry| entry.identity == identity)
                .map(|entry| entry.object.clone()))
        })?;
        if let Some(cached) = cached {
            return Ok(cached);
        }

        // The store is looked up per call rather than captured: a JS value
        // holding a handle to the store would keep the store's parked `Ctx`
        // alive past the runtime's own teardown.
        let object = Function::new(
            ctx.clone(),
            move |ctx: Ctx<'js>, Rest(arguments): Rest<Value<'js>>| {
                ExportedFunction::call(&ctx, func, &arguments)
            },
        )?;
        object.set_length(arity)?;
        if let Some(name) = name {
            object.set_name(name.to_string())?;
        }
        if let Some(object) = object.as_value().as_object() {
            object.set(Self::function_address_key(ctx)?, identity.clone())?;
        }
        Self::with(ctx, |registry| {
            registry
                .functions
                .try_borrow_mut()
                .map_err(|_| Self::busy(ctx))?
                .push(ExportedFunction {
                    identity,
                    func,
                    object: object.clone(),
                });
            Ok(())
        })?;
        Ok(object)
    }

    /// The registry of this context, installed on first use.
    ///
    /// Lazily rather than from `den:wasm`'s module initialiser so that the
    /// value layer needs nothing wired up beyond the store — the wrappers'
    /// own tests included.
    fn with<R>(ctx: &Ctx<'js>, f: impl FnOnce(&Self) -> Result<R>) -> Result<R> {
        if ctx.userdata::<Self>().is_none() {
            ctx.store_userdata(Self::default())
                .map_err(|_| Self::busy(ctx))?;
        }
        let registry = ctx.userdata::<Self>().ok_or_else(|| {
            Exception::throw_internal(
                ctx,
                "the WebAssembly reference registry is missing from this context",
            )
        })?;
        f(&registry)
    }

    /// Nothing here runs JS while a registry cell is borrowed, so this is a
    /// belt-and-braces answer rather than a reachable state — but a `RefCell`
    /// on a JS-reachable path may not be allowed to panic.
    fn busy(ctx: &Ctx<'js>) -> rquickjs::Error {
        Exception::throw_internal(ctx, "the WebAssembly reference registry is already in use")
    }
}

/// The JS wrappers for every `Memory` / `Table` / `Global` this context has
/// handed out.
///
/// The spec caches these by store address so that importing a wrapper and
/// exporting it again yields the *same* JS object. Wasmtime exposes no handle
/// equality API; its derived `Debug` identity contains the store/instance and
/// index without reading the `repr(C)` padding bytes.
#[derive(Default)]
pub struct HostWrappers<'js> {
    memories: RefCell<Vec<(String, Value<'js>)>>,
    tables:   RefCell<Vec<(String, Value<'js>)>>,
    globals:  RefCell<Vec<(String, Value<'js>)>>,
}

// SAFETY: the only `'js` data is the cached wrappers, which change lifetime
// with the struct. The wasmtime handles are store indices, not JS values.
unsafe impl<'js> JsLifetime<'js> for HostWrappers<'js> {
    type Changed<'to> = HostWrappers<'to>;
}

impl<'js> HostWrappers<'js> {
    pub fn memory(ctx: &Ctx<'js>, handle: wasmtime::Memory) -> Result<Value<'js>> {
        Self::wrap(
            ctx,
            handle,
            |wrappers| &wrappers.memories,
            |inner| crate::memory::Memory { inner }.into_js(ctx),
        )
    }

    pub fn table(ctx: &Ctx<'js>, handle: wasmtime::Table) -> Result<Value<'js>> {
        Self::wrap(
            ctx,
            handle,
            |wrappers| &wrappers.tables,
            |inner| crate::table::Table { inner }.into_js(ctx),
        )
    }

    pub fn global(ctx: &Ctx<'js>, handle: wasmtime::Global) -> Result<Value<'js>> {
        Self::wrap(
            ctx,
            handle,
            |wrappers| &wrappers.globals,
            |inner| crate::global::Global { inner }.into_js(ctx),
        )
    }

    pub fn remember_memory(
        ctx: &Ctx<'js>, handle: wasmtime::Memory, object: Value<'js>,
    ) -> Result<()> {
        Self::remember(ctx, handle, object, |wrappers| &wrappers.memories)
    }

    pub fn remember_table(
        ctx: &Ctx<'js>, handle: wasmtime::Table, object: Value<'js>,
    ) -> Result<()> {
        Self::remember(ctx, handle, object, |wrappers| &wrappers.tables)
    }

    pub fn remember_global(
        ctx: &Ctx<'js>, handle: wasmtime::Global, object: Value<'js>,
    ) -> Result<()> {
        Self::remember(ctx, handle, object, |wrappers| &wrappers.globals)
    }

    fn wrap<H: Copy + core::fmt::Debug>(
        ctx: &Ctx<'js>, handle: H, slot: impl Fn(&Self) -> &RefCell<Vec<(String, Value<'js>)>>,
        create: impl FnOnce(H) -> Result<Value<'js>>,
    ) -> Result<Value<'js>> {
        let identity = format!("{handle:?}");
        if let Some(existing) = Self::find(ctx, &identity, &slot)? {
            return Ok(existing);
        }
        let object = create(handle)?;
        Self::remember_identity(ctx, identity, object.clone(), slot)?;
        Ok(object)
    }

    fn find(
        ctx: &Ctx<'js>, identity: &str, slot: impl Fn(&Self) -> &RefCell<Vec<(String, Value<'js>)>>,
    ) -> Result<Option<Value<'js>>> {
        Self::with(ctx, |wrappers| {
            Ok(slot(wrappers)
                .try_borrow()
                .map_err(|_| Self::busy(ctx))?
                .iter()
                .find(|(cached, _)| cached == identity)
                .map(|(_, object)| object.clone()))
        })
    }

    fn remember<H: core::fmt::Debug>(
        ctx: &Ctx<'js>, handle: H, object: Value<'js>,
        slot: impl Fn(&Self) -> &RefCell<Vec<(String, Value<'js>)>>,
    ) -> Result<()> {
        let identity = format!("{handle:?}");
        Self::remember_identity(ctx, identity, object, slot)
    }

    fn remember_identity(
        ctx: &Ctx<'js>, identity: String, object: Value<'js>,
        slot: impl Fn(&Self) -> &RefCell<Vec<(String, Value<'js>)>>,
    ) -> Result<()> {
        Self::with(ctx, |wrappers| {
            let mut entries = slot(wrappers)
                .try_borrow_mut()
                .map_err(|_| Self::busy(ctx))?;
            if !entries.iter().any(|(cached, _)| cached == &identity) {
                entries.push((identity, object));
            }
            Ok(())
        })
    }

    fn with<R>(ctx: &Ctx<'js>, f: impl FnOnce(&Self) -> Result<R>) -> Result<R> {
        if ctx.userdata::<Self>().is_none() {
            ctx.store_userdata(Self::default())
                .map_err(|_| Self::busy(ctx))?;
        }
        let wrappers = ctx.userdata::<Self>().ok_or_else(|| {
            Exception::throw_internal(
                ctx,
                "the WebAssembly wrapper registry is missing from this context",
            )
        })?;
        f(&wrappers)
    }

    fn busy(ctx: &Ctx<'js>) -> rquickjs::Error {
        Exception::throw_internal(ctx, "the WebAssembly wrapper registry is already in use")
    }
}

impl<'js> ExportedFunction<'js> {
    /// "Call an Exported Function": pad the arguments with `undefined`, coerce
    /// against the declared types, then adapt the results by arity.
    ///
    /// The one implementation, shared by every Exported Function den hands out
    /// — a module's export and a `funcref` read out of a table are the same
    /// object, so they cannot be called by two different algorithms.
    fn call(ctx: &Ctx<'js>, func: Func, arguments: &[Value<'js>]) -> Result<Value<'js>> {
        let store = Store::from_ctx(ctx)?;
        // The signature is read under its own short borrow: coercion below can
        // run arbitrary JS, which must not happen while the store is borrowed.
        let signature = store.with_context(ctx, |backend_store| Ok(func.ty(&backend_store)))?;
        let undefined = Value::new_undefined(ctx.clone());
        let parameters = Vec::from_iter(signature.params())
            .iter()
            .enumerate()
            .map(|(position, ty)| {
                let argument = arguments.get(position).unwrap_or(&undefined);
                WasmValue::from_js(ctx, argument, ty).map(WasmValue::into_inner)
            })
            .collect::<Result<Vec<_>>>()?;
        // `v128` has no JS representation, so a signature mentioning it is a
        // `TypeError` on every call — which is what `default_for` reports here
        // and `from_js` reports above.
        let mut results = Vec::from_iter(signature.results())
            .iter()
            .map(|ty| {
                WasmValue::default_for(ty)
                    .map(WasmValue::into_inner)
                    .map_err(|err| Exception::throw_type(ctx, err))
            })
            .collect::<Result<Vec<_>>>()?;

        let called = store
            .invoke(ctx, &func, &parameters, &mut results)
            .map_err(|err| Self::throw_call_failure(ctx, err));
        // "Refresh the memory buffer" on the way out, for a trap as well as a
        // return: a call that trapped may still have grown a memory first. The
        // trap is the more useful error of the two, so it is the one reported.
        let refreshed = MemoryBuffers::refresh(ctx);
        called?;
        refreshed?;

        let results = results
            .into_iter()
            .map(|value| WasmValue(value).to_js(ctx))
            .collect::<Result<Vec<_>>>()?;
        match results.as_slice() {
            [] => Ok(Value::new_undefined(ctx.clone())),
            [single] => Ok(single.clone()),
            _ => {
                let array = Array::new(ctx.clone())?;
                for (position, value) in results.into_iter().enumerate() {
                    array.set(position, value)?;
                }
                Ok(array.into_value())
            }
        }
    }

    /// A trap that started life as a JS exception thrown by an imported
    /// function has to reach the caller as that same object, so a still-pending
    /// exception wins over the engine's trap description.
    fn throw_call_failure(ctx: &Ctx<'js>, error: wasmtime::Error) -> rquickjs::Error {
        // `Ctx::catch` is `JS_GetException`, which hands back `JS_UNINITIALIZED`
        // when nothing is pending — neither `undefined` nor `null`, so the tag
        // cannot be used to decide this.
        if ctx.has_exception() {
            rquickjs::Error::Exception
        } else {
            throw_runtime_error(ctx, error)
        }
    }
}

#[cfg(test)]
#[path = "../tests/unit/utils.rs"]
mod tests;
