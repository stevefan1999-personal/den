//! WHATWG Streams.
//!
//! The state machine is synchronous Rust. Promises are minted with
//! `ctx.promise()` and settled inline, so their reactions land on QuickJS's own
//! microtask queue and turn ordering matches the specification. `ctx.spawn` is
//! confined to [`native`], where a host byte sink is driven.
//!
//! Internal sequencing that the specification expresses as records rather than
//! promises — read requests, the pipe and tee consumers — stays Rust closures,
//! so piping never creates a `{ value, done }` object and never touches a
//! user-visible `then`.

pub mod native;
mod pipe;
mod readable;
mod strategy;
mod transform;
mod writable;

use rquickjs::{
    Ctx, Function, IntoJs, JsLifetime, Object, Promise, Result, Value,
    class::{Trace, Tracer},
    function::This,
};

pub use crate::streams::{
    native::{ByteSink, SinkFuture},
    readable::{
        ReadableStream, ReadableStreamAsyncIterator, ReadableStreamDefaultController,
        ReadableStreamDefaultReader,
    },
    strategy::{ByteLengthQueuingStrategy, CountQueuingStrategy},
    transform::{TransformStream, TransformStreamDefaultController},
    writable::{WritableStream, WritableStreamDefaultController, WritableStreamDefaultWriter},
};

/// `Promise.prototype.then`, `Function.prototype.bind` and a no-op, captured
/// before user code can patch them. Internal reactions go through these, so
/// patching a prototype neither breaks nor observes the stream machinery.
#[derive(JsLifetime, Clone)]
pub(crate) struct Intrinsics<'js> {
    then:      Function<'js>,
    bind:      Function<'js>,
    noop:      Function<'js>,
    /// The queuing strategies' `size` is one function per realm, not one per
    /// instance: `queuing-strategies.any.js` compares them by identity.
    count:     Function<'js>,
    byte_size: Function<'js>,
}

pub fn install_intrinsics(ctx: &Ctx<'_>) -> Result<()> {
    let promise: Object = ctx.globals().get("Promise")?;
    let proto: Object = promise.get("prototype")?;
    let then: Function = proto.get("then")?;
    let function: Object = ctx.globals().get("Function")?;
    let bind: Function = function.get::<_, Object>("prototype")?.get("bind")?;
    let noop = Function::new(ctx.clone(), || {})?;
    let count = Function::new(ctx.clone(), || 1.0)?.with_name("size")?;
    // JavaScript property access performs ToObject for primitives, while
    // preserving the TypeError for null/undefined and any getter exception.
    let byte_size = ctx
        .eval::<Function, _>("chunk => chunk.byteLength")?
        .with_name("size")?;
    let _ = ctx.store_userdata(Intrinsics {
        then,
        bind,
        noop,
        count,
        byte_size,
    });
    Ok(())
}

fn intrinsics<'js>(ctx: &Ctx<'js>) -> Result<Intrinsics<'js>> {
    if ctx.userdata::<Intrinsics<'js>>().is_none() {
        install_intrinsics(ctx)?;
    }
    let cached = ctx
        .userdata::<Intrinsics<'js>>()
        .ok_or_else(|| Exception::throw_type(ctx, "stream intrinsics are unavailable"))?;
    // Deref explicitly: the guard implements no `Clone` of its own today, and a
    // future one must not silently start cloning the guard instead.
    Ok((*cached).clone())
}

/// PerformPromiseThen over the pristine `then`. `value` is treated as the
/// specification's `promiseResolve(value)`: an existing promise is reacted to
/// directly, anything else is wrapped so the reaction still costs one turn.
pub(crate) fn react<'js>(
    ctx: &Ctx<'js>, value: Value<'js>, on_ok: Option<Function<'js>>, on_err: Option<Function<'js>>,
) -> Result<()> {
    let then = intrinsics(ctx)?.then;
    let promise = if value.is_promise() {
        value
    } else {
        Cap::resolved(ctx, value)?.into_value()
    };
    then.call::<_, Value>((This(promise), on_ok, on_err))?;
    Ok(())
}

/// A one-turn identity chain using the pristine `Promise.prototype.then`.
pub(crate) fn chain<'js>(ctx: &Ctx<'js>, promise: Promise<'js>) -> Result<Promise<'js>> {
    let then = intrinsics(ctx)?.then;
    then.call((This(promise),))
}

/// Chain a user-returned value onto a fresh promise that fulfils with
/// `undefined` and forwards a rejection unchanged. The reaction handlers are
/// the new promise's own capability functions, so no Rust record holding JS
/// values has to survive the reaction.
pub(crate) fn chain_undefined<'js>(ctx: &Ctx<'js>, value: Value<'js>) -> Result<Promise<'js>> {
    let (promise, resolve, reject) = ctx.promise()?;
    let ok = Function::new(ctx.clone(), move || {
        let _ = resolve.call::<_, ()>(());
    })?;
    react(ctx, value, Some(ok), Some(reject))?;
    Ok(promise)
}

/// Attach a no-op rejection handler so a promise nobody observes cannot trip
/// den's unhandled-rejection tracker.
///
/// Goes through the realm's pristine `then`, so a script that patches
/// `Promise.prototype.then` cannot observe or break an internal reaction —
/// which is why this is public rather than something each caller reimplements.
pub fn mark_handled<'js>(ctx: &Ctx<'js>, promise: &Promise<'js>) {
    if let Ok(cached) = intrinsics(ctx) {
        let _ = cached.then.call::<_, Value>((
            This(promise.clone()),
            Option::<Function>::None,
            cached.noop,
        ));
    }
}

/// Bind `keeper` onto `function` as a leading argument, through the realm's
/// pristine `Function.prototype.bind`.
///
/// A reaction implemented with `Function::new` cannot hold a JS value itself:
/// `RustFunction` traces nothing, so a handle it captures is a refcount the
/// collector cannot follow — an external root still standing when
/// `JS_FreeRuntime` asserts every object is gone. Riding on the bound function
/// object instead is an edge the collector can follow, and the value dies with
/// the reaction.
pub(crate) fn bound<'js, K: IntoJs<'js>>(
    ctx: &Ctx<'js>, function: Function<'js>, keeper: K,
) -> Result<Function<'js>> {
    intrinsics(ctx)?
        .bind
        .call((This(function), Value::new_undefined(ctx.clone()), keeper))
}

/// A promise plus its capability functions. Settling drops both functions,
/// which is what breaks the pending-reaction cycle at settle time.
pub(crate) struct Cap<'js> {
    promise: Promise<'js>,
    resolve: Option<Function<'js>>,
    reject:  Option<Function<'js>>,
}

impl<'js> Trace<'js> for Cap<'js> {
    fn trace<'a>(&self, tracer: Tracer<'a, 'js>) {
        self.promise.trace(tracer);
        self.resolve.trace(tracer);
        self.reject.trace(tracer);
    }
}

impl<'js> Cap<'js> {
    pub(crate) fn new(ctx: &Ctx<'js>) -> Result<Self> {
        let (promise, resolve, reject) = ctx.promise()?;
        Ok(Self {
            promise,
            resolve: Some(resolve),
            reject: Some(reject),
        })
    }

    pub(crate) fn promise(&self) -> Promise<'js> { self.promise.clone() }

    pub(crate) const fn is_pending(&self) -> bool { self.resolve.is_some() }

    pub(crate) fn resolve(&mut self, value: Value<'js>) {
        self.reject = None;
        if let Some(resolve) = self.resolve.take() {
            let _ = resolve.call::<_, ()>((value,));
        }
    }

    pub(crate) fn fulfill(&mut self, ctx: &Ctx<'js>) {
        self.resolve(Value::new_undefined(ctx.clone()));
    }

    pub(crate) fn reject(&mut self, value: Value<'js>) {
        self.resolve = None;
        if let Some(reject) = self.reject.take() {
            let _ = reject.call::<_, ()>((value,));
        }
    }

    /// Reject and immediately mark handled: used for promises the caller may
    /// never look at (`reader.closed` after `releaseLock`, and friends).
    pub(crate) fn reject_handled(&mut self, ctx: &Ctx<'js>, value: Value<'js>) {
        let promise = self.promise.clone();
        self.reject(value);
        mark_handled(ctx, &promise);
    }

    /// Reject, replacing an already-settled capability with a fresh one first:
    /// a writer's `ready`/`closed` must show the new failure even after they
    /// fulfilled.
    pub(crate) fn reject_or_replace(&mut self, ctx: &Ctx<'js>, reason: Value<'js>) {
        if !self.is_pending()
            && let Ok(fresh) = Self::new(ctx)
        {
            *self = fresh;
        }
        self.reject_handled(ctx, reason);
    }

    /// Hand the capability functions to promise reactions directly, so no Rust
    /// record has to keep them alive.
    pub(crate) fn into_parts(self) -> (Option<Function<'js>>, Option<Function<'js>>) {
        (self.resolve, self.reject)
    }

    pub(crate) fn resolved(ctx: &Ctx<'js>, value: Value<'js>) -> Result<Promise<'js>> {
        let mut cap = Self::new(ctx)?;
        let promise = cap.promise();
        cap.resolve(value);
        Ok(promise)
    }

    pub(crate) fn undefined(ctx: &Ctx<'js>) -> Result<Promise<'js>> {
        Self::resolved(ctx, Value::new_undefined(ctx.clone()))
    }

    pub(crate) fn rejected(ctx: &Ctx<'js>, reason: Value<'js>) -> Result<Promise<'js>> {
        let mut cap = Self::new(ctx)?;
        let promise = cap.promise();
        cap.reject(reason);
        Ok(promise)
    }
}

/// The value a thrown `rquickjs::Error` carries, as a JS value.
pub(crate) fn thrown<'js>(ctx: &Ctx<'js>, error: rquickjs::Error) -> Value<'js> {
    match error {
        rquickjs::Error::Exception => ctx.catch(),
        other => {
            den_util::stack::error_from_message(ctx, &other.to_string()).map_or_else(
                |_| Value::new_undefined(ctx.clone()),
                |exception| exception.into_object().into_value(),
            )
        }
    }
}

pub(crate) fn type_error<'js>(ctx: &Ctx<'js>, message: &str) -> Value<'js> {
    let error = Exception::throw_type(ctx, message);
    thrown(ctx, error)
}

pub(crate) fn range_error<'js>(ctx: &Ctx<'js>, message: &str) -> Value<'js> {
    let error = Exception::throw_range(ctx, message);
    thrown(ctx, error)
}

/// WebIDL `optional object x = {}`: an omitted or undefined argument is the
/// default, and anything present that is not an object — `null` above all — is
/// a TypeError rather than a silently empty bag.
pub(crate) fn optional_object<'js>(
    ctx: &Ctx<'js>, value: Option<Value<'js>>, what: &str,
) -> Result<Object<'js>> {
    match value {
        None => Object::new(ctx.clone()),
        Some(value) if value.is_undefined() => Object::new(ctx.clone()),
        Some(value) => {
            value
                .into_object()
                .ok_or_else(|| Exception::throw_type(ctx, &format!("{what} must be an object")))
        }
    }
}
/// Get a method off an options bag exactly once, per the specification's
/// "let x be ? GetV(...)" steps. A present-but-uncallable member is a
/// TypeError.
pub(crate) fn method<'js>(
    ctx: &Ctx<'js>, object: &Object<'js>, name: &str,
) -> Result<Option<Function<'js>>> {
    let value: Value = object.get(name)?;
    if value.is_undefined() || value.is_null() {
        return Ok(None);
    }
    value.into_function().map(Some).ok_or_else(|| {
        Exception::throw_type(
            ctx,
            &format!("underlying stream member `{name}` is not callable"),
        )
    })
}

use rquickjs::Exception;
