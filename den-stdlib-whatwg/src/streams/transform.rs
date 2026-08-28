//! `TransformStream` and its default controller.
//!
//! The writable half's sink runs the transformer; the readable half's pull
//! clears backpressure. That pair is what makes a slow reader slow the writer.

use std::{
    cell::RefCell,
    rc::{Rc, Weak},
};

use rquickjs::{
    Class, Ctx, Exception, Function, JsLifetime, Object, Promise, Result, Value,
    atom::PredefinedAtom,
    class::{Trace, Tracer},
    function::{Opt, This},
};

use crate::streams::{
    Cap, method, react,
    readable::{Inner as RsInner, ReadableStream, extract_strategy},
    thrown, type_error,
    writable::{Inner as WsInner, WritableStream},
};

struct TransformInner<'js> {
    transformer:         Object<'js>,
    transform_fn:        Option<Function<'js>>,
    flush_fn:            Option<Function<'js>>,
    cancel_fn:           Option<Function<'js>>,
    /// Weak on both halves: each half's `Rc` is owned by its stream class, and
    /// each half owns a closure that owns this record. Strong links here would
    /// close a cycle that nothing can break.
    readable:            Weak<RefCell<crate::streams::readable::ReadableInner<'js>>>,
    writable:            Weak<RefCell<crate::streams::writable::WritableInner<'js>>>,
    backpressure:        bool,
    backpressure_change: Option<Cap<'js>>,
    /// Writes parked on `backpressure_change`. The reaction that resumes one
    /// is a `Function::new` closure, and rquickjs gives `RustFunction` an empty
    /// `Trace` impl — so such a closure may capture no JS value or the
    /// collector cannot see it and the whole graph outlives the runtime. The
    /// chunk and its capability live here, traced, and the closure carries
    /// nothing but an id.
    parked:              Vec<(u64, Value<'js>, Cap<'js>)>,
    next_parked:         u64,
}

// SAFETY: see the matching impl in `readable`.
unsafe impl<'js> rquickjs::JsLifetime<'js> for TransformInner<'js> {
    type Changed<'to> = TransformInner<'to>;
}

impl<'js> Trace<'js> for TransformInner<'js> {
    fn trace<'a>(&self, tracer: Tracer<'a, 'js>) {
        self.transformer.trace(tracer);
        self.transform_fn.trace(tracer);
        self.flush_fn.trace(tracer);
        self.cancel_fn.trace(tracer);
        self.backpressure_change
            .as_ref()
            .map(|cap| cap.trace(tracer));
        for (_, chunk, cap) in &self.parked {
            chunk.trace(tracer);
            cap.trace(tracer);
        }
    }
}

/// Owned by the controller; every other holder keeps a weak handle.
type Owned<'js> = Rc<RefCell<TransformInner<'js>>>;
type Shared<'js> = Weak<RefCell<TransformInner<'js>>>;

fn readable_of<'js>(shared: &Shared<'js>) -> Option<RsInner<'js>> {
    shared.upgrade()?.borrow().readable.upgrade()
}

fn writable_of<'js>(shared: &Shared<'js>) -> Option<WsInner<'js>> {
    shared.upgrade()?.borrow().writable.upgrade()
}

/// The controller object lives in the readable half's `source` slot, which the
/// readable stream traces. That keeps it collectable while still letting the
/// transformer reach it.
fn controller_of<'js>(
    shared: &Shared<'js>,
) -> Option<Class<'js, TransformStreamDefaultController<'js>>> {
    let readable = readable_of(shared)?;
    let source = readable.borrow().source.clone();
    Class::<TransformStreamDefaultController>::from_object(&source)
}

#[derive(JsLifetime)]
#[rquickjs::class]
pub struct TransformStream<'js> {
    #[qjs(get)]
    pub(crate) readable: Class<'js, ReadableStream<'js>>,
    #[qjs(get)]
    pub(crate) writable: Class<'js, WritableStream<'js>>,
}

/// `shared` is owned by the algorithm closures inside each half, and is traced
/// by none of them: a record with several strong Rust owners must be traced by
/// no owner, or the collector would free values another owner still holds.
impl<'js> Trace<'js> for TransformStream<'js> {
    fn trace<'a>(&self, tracer: Tracer<'a, 'js>) {
        self.readable.trace(tracer);
        self.writable.trace(tracer);
    }
}

fn set_backpressure<'js>(ctx: &Ctx<'js>, shared: &Shared<'js>, backpressure: bool) {
    let Some(shared) = shared.upgrade() else {
        return;
    };
    if let Some(mut cap) = shared.borrow_mut().backpressure_change.take() {
        cap.fulfill(ctx);
    }
    let mut borrow = shared.borrow_mut();
    borrow.backpressure_change = Cap::new(ctx).ok();
    borrow.backpressure = backpressure;
}

/// Error both halves. A transformer that throws must not leave readers hanging.
fn error_both<'js>(ctx: &Ctx<'js>, shared: &Shared<'js>, reason: Value<'js>) {
    if let Some(readable) = readable_of(shared) {
        ReadableStream::error(ctx, &readable, reason.clone());
    }
    if let Some(writable) = writable_of(shared) {
        WritableStream::start_erroring(ctx, &writable, reason);
    }
}

/// Resume a write that was parked on backpressure, settling the capability the
/// sink handed back with the transform's own outcome.
fn resume_parked<'js>(ctx: &Ctx<'js>, shared: &Shared<'js>, id: u64) {
    let Some(owned) = shared.upgrade() else {
        return;
    };
    let parked = {
        let mut borrow = owned.borrow_mut();
        borrow
            .parked
            .iter()
            .position(|(each, ..)| *each == id)
            .map(|at| borrow.parked.remove(at))
    };
    let Some((_, chunk, mut cap)) = parked else {
        return;
    };
    match perform_transform(ctx, shared, chunk) {
        Ok(transformed) => {
            let (resolve, reject) = cap.into_parts();
            let ok = resolve.and_then(|resolve| {
                Function::new(ctx.clone(), move || {
                    let _ = resolve.call::<_, ()>(());
                })
                .ok()
            });
            let _ = react(ctx, transformed.into_value(), ok, reject);
        }
        Err(error) => cap.reject(thrown(ctx, error)),
    }
}

fn perform_transform<'js>(
    ctx: &Ctx<'js>, shared: &Shared<'js>, chunk: Value<'js>,
) -> Result<Promise<'js>> {
    let Some(owned) = shared.upgrade() else {
        return Cap::undefined(ctx);
    };
    let (transform_fn, transformer) = {
        let borrow = owned.borrow();
        (borrow.transform_fn.clone(), borrow.transformer.clone())
    };
    let controller = controller_of(shared);
    let Some(transform) = transform_fn else {
        let readable = readable_of(shared);
        if let Some(readable) = readable
            && let Err(error) = ReadableStream::enqueue(ctx, &readable, chunk)
        {
            let reason = thrown(ctx, error);
            error_both(ctx, shared, reason.clone());
            return Cap::rejected(ctx, reason);
        }
        return Cap::undefined(ctx);
    };
    let outcome = match controller {
        Some(controller) => transform.call::<_, Value>((This(transformer), chunk, controller)),
        None => Ok(Value::new_undefined(ctx.clone())),
    };
    let value = match outcome {
        Ok(value) => value,
        Err(error) => {
            let reason = thrown(ctx, error);
            error_both(ctx, shared, reason.clone());
            return Cap::rejected(ctx, reason);
        }
    };
    let (promise, resolve, reject) = ctx.promise()?;
    let on_ok = Function::new(ctx.clone(), move || {
        let _ = resolve.call::<_, ()>(());
    })?;
    let on_err = {
        let shared = shared.clone();
        Function::new(ctx.clone(), move |ctx: Ctx<'js>, reason: Value<'js>| {
            error_both(&ctx, &shared, reason.clone());
            let _ = reject.call::<_, ()>((reason,));
        })?
    };
    react(ctx, value, Some(on_ok), Some(on_err))?;
    Ok(promise)
}

#[rquickjs::methods(rename_all = "camelCase")]
impl<'js> TransformStream<'js> {
    #[qjs(constructor)]
    pub fn new(
        ctx: Ctx<'js>, transformer: Opt<Value<'js>>, writable_strategy: Opt<Value<'js>>,
        readable_strategy: Opt<Value<'js>>,
    ) -> Result<Self> {
        let transformer_object = match transformer.0 {
            Some(value) if value.is_object() => {
                value.into_object().unwrap_or(Object::new(ctx.clone())?)
            }
            _ => Object::new(ctx.clone())?,
        };
        for reserved in ["readableType", "writableType"] {
            let value: Value = transformer_object.get(reserved)?;
            if !value.is_undefined() {
                return Err(Exception::throw_range(
                    &ctx,
                    &format!("transformer.{reserved} is reserved and must be undefined"),
                ));
            }
        }
        let start_fn = method(&ctx, &transformer_object, "start")?;
        let transform_fn = method(&ctx, &transformer_object, "transform")?;
        let flush_fn = method(&ctx, &transformer_object, "flush")?;
        let cancel_fn = method(&ctx, &transformer_object, "cancel")?;
        let (writable_hwm, writable_size) = extract_strategy(&ctx, writable_strategy.0, 1.0)?;
        let (readable_hwm, readable_size) = extract_strategy(&ctx, readable_strategy.0, 0.0)?;

        let owned: Owned<'js> = Rc::new(RefCell::new(TransformInner {
            transformer: transformer_object.clone(),
            transform_fn,
            flush_fn,
            cancel_fn,
            readable: Weak::new(),
            writable: Weak::new(),
            backpressure: false,
            backpressure_change: None,
            parked: Vec::new(),
            next_parked: 0,
        }));
        let shared: Shared<'js> = Rc::downgrade(&owned);
        // The specification starts a transform under backpressure and lets the
        // readable half's first pull clear it. That has to happen before the
        // readable exists, or attaching its controller pulls, clears the flag,
        // and this call puts it straight back on with no pull left to lift it.
        set_backpressure(&ctx, &shared, true);

        // Readable half: pulling clears backpressure, cancelling errors the
        // writable half.
        let readable_inner = ReadableStream::new_inner(&ctx)?;
        {
            let mut borrow = readable_inner.borrow_mut();
            borrow.started = true;
            borrow.hwm = readable_hwm;
            borrow.size_fn = readable_size;
            borrow.pull_fn = Some(Function::new(ctx.clone(), {
                let shared = shared.clone();
                move |ctx: Ctx<'js>| set_backpressure(&ctx, &shared, false)
            })?);
            borrow.cancel_fn = Some(Function::new(ctx.clone(), {
                let shared = shared.clone();
                move |ctx: Ctx<'js>, reason: Opt<Value<'js>>| -> Result<Value<'js>> {
                    let reason = reason
                        .0
                        .unwrap_or_else(|| Value::new_undefined(ctx.clone()));
                    let (cancel_fn, transformer) = match shared.upgrade() {
                        Some(owned) => {
                            let borrow = owned.borrow();
                            (borrow.cancel_fn.clone(), borrow.transformer.clone())
                        }
                        None => (None, Object::new(ctx.clone())?),
                    };
                    if let Some(writable) = writable_of(&shared) {
                        WritableStream::start_erroring(&ctx, &writable, reason.clone());
                    }
                    match cancel_fn {
                        Some(cancel) => cancel.call((This(transformer), reason)),
                        None => Ok(Value::new_undefined(ctx.clone())),
                    }
                }
            })?);
        }
        owned.borrow_mut().readable = Rc::downgrade(&readable_inner);
        ReadableStream::attach_controller(&ctx, &readable_inner)?;
        let readable = Class::instance(ctx.clone(), ReadableStream {
            inner: Rc::clone(&readable_inner),
        })?;

        // Writable half: its sink is the transformer.
        let writable_inner = WritableStream::new_inner(&ctx)?;
        let sink = Object::new(ctx.clone())?;
        sink.set(
            "write",
            Function::new(ctx.clone(), {
                let shared = shared.clone();
                move |ctx: Ctx<'js>, chunk: Value<'js>| -> Result<Promise<'js>> {
                    let Some(owned) = shared.upgrade() else {
                        return Cap::undefined(&ctx);
                    };
                    if !owned.borrow().backpressure {
                        return perform_transform(&ctx, &shared, chunk);
                    }
                    let waiting = owned
                        .borrow()
                        .backpressure_change
                        .as_ref()
                        .map(Cap::promise);
                    let Some(waiting) = waiting else {
                        return perform_transform(&ctx, &shared, chunk);
                    };
                    let cap = Cap::new(&ctx)?;
                    let promise = cap.promise();
                    let id = {
                        let mut borrow = owned.borrow_mut();
                        let id = borrow.next_parked;
                        borrow.next_parked += 1;
                        borrow.parked.push((id, chunk, cap));
                        id
                    };
                    let on_ok = {
                        let shared = shared.clone();
                        Function::new(ctx.clone(), move |ctx: Ctx<'js>| {
                            resume_parked(&ctx, &shared, id);
                        })?
                    };
                    react(&ctx, waiting.into_value(), Some(on_ok), None)?;
                    Ok(promise)
                }
            })?,
        )?;
        sink.set(
            "close",
            Function::new(ctx.clone(), {
                let shared = shared.clone();
                move |ctx: Ctx<'js>| -> Result<Promise<'js>> {
                    let (flush_fn, transformer) = match shared.upgrade() {
                        Some(owned) => {
                            let borrow = owned.borrow();
                            (borrow.flush_fn.clone(), borrow.transformer.clone())
                        }
                        None => (None, Object::new(ctx.clone())?),
                    };
                    let controller = controller_of(&shared);
                    let readable = readable_of(&shared);
                    let flushed = match (flush_fn, controller) {
                        (Some(flush), Some(controller)) => {
                            match flush.call::<_, Value>((This(transformer), controller)) {
                                Ok(value) => value,
                                Err(error) => {
                                    let reason = thrown(&ctx, error);
                                    error_both(&ctx, &shared, reason.clone());
                                    return Cap::rejected(&ctx, reason);
                                }
                            }
                        }
                        _ => Value::new_undefined(ctx.clone()),
                    };
                    let (promise, resolve, reject) = ctx.promise()?;
                    let on_ok = {
                        let readable = readable.clone();
                        let reject = reject.clone();
                        Function::new(ctx.clone(), move |ctx: Ctx<'js>| {
                            if let Some(readable) = readable.as_ref() {
                                if let Some(reason) = ReadableStream::stored_error(readable) {
                                    let _ = reject.call::<_, ()>((reason,));
                                    return;
                                }
                                let _ = ReadableStream::close_requested(&ctx, readable);
                            }
                            let _ = resolve.call::<_, ()>(());
                        })?
                    };
                    let on_err = {
                        let shared = shared.clone();
                        Function::new(ctx.clone(), move |ctx: Ctx<'js>, reason: Value<'js>| {
                            error_both(&ctx, &shared, reason.clone());
                            let _ = reject.call::<_, ()>((reason,));
                        })?
                    };
                    react(&ctx, flushed, Some(on_ok), Some(on_err))?;
                    Ok(promise)
                }
            })?,
        )?;
        sink.set(
            "abort",
            Function::new(ctx.clone(), {
                let shared = shared.clone();
                move |ctx: Ctx<'js>, reason: Opt<Value<'js>>| -> Result<Value<'js>> {
                    let reason = reason
                        .0
                        .unwrap_or_else(|| Value::new_undefined(ctx.clone()));
                    let (cancel_fn, transformer) = match shared.upgrade() {
                        Some(owned) => {
                            let borrow = owned.borrow();
                            (borrow.cancel_fn.clone(), borrow.transformer.clone())
                        }
                        None => (None, Object::new(ctx.clone())?),
                    };
                    if let Some(readable) = readable_of(&shared) {
                        ReadableStream::error(&ctx, &readable, reason.clone());
                    }
                    match cancel_fn {
                        Some(cancel) => cancel.call((This(transformer), reason)),
                        None => Ok(Value::new_undefined(ctx.clone())),
                    }
                }
            })?,
        )?;
        owned.borrow_mut().writable = Rc::downgrade(&writable_inner);
        {
            let mut borrow = writable_inner.borrow_mut();
            borrow.hwm = writable_hwm;
            borrow.size_fn = writable_size;
        }
        WritableStream::setup_with_sink(&ctx, &writable_inner, sink, writable_hwm)?;
        let writable = Class::instance(ctx.clone(), WritableStream {
            inner: Rc::clone(&writable_inner),
        })?;

        let controller = Class::instance(ctx.clone(), TransformStreamDefaultController {
            shared: owned,
        })?;
        // Both halves root the controller, which owns the shared record.
        readable_inner
            .borrow_mut()
            .roots
            .push(controller.clone().into_value());
        writable_inner
            .borrow_mut()
            .roots
            .push(controller.clone().into_value());
        readable_inner.borrow_mut().source = controller.clone().into_inner();

        let started = match start_fn {
            Some(start) => start.call::<_, Value>((This(transformer_object), controller))?,
            None => Value::new_undefined(ctx.clone()),
        };
        let on_err = {
            let shared = shared.clone();
            Function::new(ctx.clone(), move |ctx: Ctx<'js>, reason: Value<'js>| {
                error_both(&ctx, &shared, reason);
            })?
        };
        // The readable half's controller starts already-started, so nothing has
        // kicked its first pull. Do it when the transformer's `start` settles,
        // where the specification's start promise would: a readable strategy
        // with room pulls straight away and lifts the initial backpressure.
        let on_ok = {
            let readable_inner = Rc::clone(&readable_inner);
            Function::new(ctx.clone(), move |ctx: Ctx<'js>| {
                ReadableStream::pull_if_needed(&ctx, &readable_inner);
            })?
        };
        react(&ctx, started, Some(on_ok), Some(on_err))?;

        Ok(Self { readable, writable })
    }

    #[qjs(prop, rename = PredefinedAtom::SymbolToStringTag, configurable)]
    pub fn to_string_tag() -> &'static str { "TransformStream" }
}

/// The controller is the single strong owner of the shared transform record,
/// and therefore its single tracer. Both halves root the controller, so the
/// record outlives whichever half is dropped first.
#[rquickjs::class]
pub struct TransformStreamDefaultController<'js> {
    shared: Owned<'js>,
}

// SAFETY: every field is a `'js` handle, exactly as the derive would generate.
unsafe impl<'js> rquickjs::JsLifetime<'js> for TransformStreamDefaultController<'js> {
    type Changed<'to> = TransformStreamDefaultController<'to>;
}

impl<'js> Trace<'js> for TransformStreamDefaultController<'js> {
    fn trace<'a>(&self, tracer: Tracer<'a, 'js>) {
        if let Ok(shared) = self.shared.try_borrow() {
            shared.trace(tracer);
        }
    }
}

#[rquickjs::methods(rename_all = "camelCase")]
impl<'js> TransformStreamDefaultController<'js> {
    #[qjs(constructor)]
    pub fn new(ctx: Ctx<'js>) -> Result<Self> {
        Err(Exception::throw_type(&ctx, "Illegal constructor"))
    }

    #[qjs(get)]
    pub fn desired_size(&self, ctx: Ctx<'js>) -> Value<'js> {
        match readable_of(&Rc::downgrade(&self.shared))
            .and_then(|readable| ReadableStream::desired_size(&readable))
        {
            Some(size) => Value::new_float(ctx, size),
            None => Value::new_null(ctx),
        }
    }

    pub fn enqueue(&self, ctx: Ctx<'js>, chunk: Opt<Value<'js>>) -> Result<()> {
        let shared = Rc::downgrade(&self.shared);
        let Some(readable) = readable_of(&shared) else {
            return Ok(());
        };
        let chunk = chunk.0.unwrap_or_else(|| Value::new_undefined(ctx.clone()));
        if let Err(error) = ReadableStream::enqueue(&ctx, &readable, chunk) {
            let reason = thrown(&ctx, error);
            error_both(&ctx, &shared, reason.clone());
            return Err(ctx.throw(reason));
        }
        let backpressure = ReadableStream::desired_size(&readable).is_none_or(|size| size <= 0.0);
        if backpressure != self.shared.borrow().backpressure {
            set_backpressure(&ctx, &shared, backpressure);
        }
        Ok(())
    }

    pub fn error(&self, ctx: Ctx<'js>, reason: Opt<Value<'js>>) {
        {
            let shared = Rc::downgrade(&self.shared);
            error_both(
                &ctx,
                &shared,
                reason
                    .0
                    .unwrap_or_else(|| Value::new_undefined(ctx.clone())),
            );
        }
    }

    pub fn terminate(&self, ctx: Ctx<'js>) {
        let shared = Rc::downgrade(&self.shared);
        let (readable, writable) = (readable_of(&shared), writable_of(&shared));
        if let Some(readable) = readable {
            let _ = ReadableStream::close_requested(&ctx, &readable);
        }
        if let Some(writable) = writable {
            let reason = type_error(&ctx, "the transform stream was terminated");
            WritableStream::start_erroring(&ctx, &writable, reason);
        }
    }

    #[qjs(prop, rename = PredefinedAtom::SymbolToStringTag, configurable)]
    pub fn to_string_tag() -> &'static str { "TransformStreamDefaultController" }
}
