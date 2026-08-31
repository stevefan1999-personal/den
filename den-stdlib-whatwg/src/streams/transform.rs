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
    Cap, Pins, method, optional_object, react,
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
    finish:              Option<(Cap<'js>, FinishAction<'js>)>,
    finish_error:        Option<Value<'js>>,
}

#[derive(Clone)]
enum FinishAction<'js> {
    SourceCancel(Value<'js>),
    SinkAbort(Value<'js>),
    SinkClose,
}

impl<'js> Trace<'js> for FinishAction<'js> {
    fn trace<'a>(&self, tracer: Tracer<'a, 'js>) {
        match self {
            Self::SourceCancel(reason) | Self::SinkAbort(reason) => reason.trace(tracer),
            Self::SinkClose => {}
        }
    }
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
        if let Some(cap) = self.backpressure_change.as_ref() {
            cap.trace(tracer);
        }
        for (_, chunk, cap) in &self.parked {
            chunk.trace(tracer);
            cap.trace(tracer);
        }
        if let Some((cap, action)) = &self.finish {
            cap.trace(tracer);
            action.trace(tracer);
        }
        self.finish_error.trace(tracer);
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

/// TransformStreamErrorWritableAndUnblockWrite. A transformer that throws must
/// not leave readers hanging, and it must not leave a write parked on
/// backpressure that nothing will ever lift: erroring is what unblocks it, and
/// the write then rejects with the stream's stored error.
fn error_both<'js>(ctx: &Ctx<'js>, shared: &Shared<'js>, reason: Value<'js>) {
    if let Some(readable) = readable_of(shared) {
        ReadableStream::error(ctx, &readable, reason.clone());
    }
    if let Some(writable) = writable_of(shared) {
        WritableStream::start_erroring(ctx, &writable, reason);
    }
    clear_algorithms(shared);
    unblock_write(ctx, shared);
}

fn clear_algorithms(shared: &Shared<'_>) {
    if let Some(owned) = shared.upgrade() {
        let mut borrow = owned.borrow_mut();
        borrow.transform_fn = None;
        borrow.flush_fn = None;
        borrow.cancel_fn = None;
    }
}

fn settle_finish<'js>(ctx: &Ctx<'js>, shared: &Shared<'js>, rejected: Option<Value<'js>>) {
    let Some(owned) = shared.upgrade() else {
        return;
    };
    let Some(action) = owned
        .borrow()
        .finish
        .as_ref()
        .map(|(_, action)| action.clone())
    else {
        return;
    };
    let error = match (rejected, action) {
        (Some(reason), FinishAction::SourceCancel(_)) => {
            let reason = owned.borrow_mut().finish_error.take().unwrap_or(reason);
            if let Some(writable) = writable_of(shared) {
                WritableStream::start_erroring(ctx, &writable, reason.clone());
            }
            unblock_write(ctx, shared);
            Some(reason)
        }
        (Some(reason), FinishAction::SinkAbort(_) | FinishAction::SinkClose) => {
            if let Some(readable) = readable_of(shared) {
                ReadableStream::error(ctx, &readable, reason.clone());
            }
            Some(reason)
        }
        (None, FinishAction::SourceCancel(reason)) => {
            let error = owned.borrow_mut().finish_error.take();
            if error.is_none() {
                if let Some(writable) = writable_of(shared) {
                    WritableStream::start_erroring(ctx, &writable, reason);
                }
                unblock_write(ctx, shared);
            }
            error
        }
        (None, FinishAction::SinkAbort(reason)) => {
            let error =
                readable_of(shared).and_then(|readable| ReadableStream::stored_error(&readable));
            if error.is_none()
                && let Some(readable) = readable_of(shared)
            {
                ReadableStream::error(ctx, &readable, reason);
            }
            error
        }
        (None, FinishAction::SinkClose) => {
            let error =
                readable_of(shared).and_then(|readable| ReadableStream::stored_error(&readable));
            if error.is_none()
                && let Some(readable) = readable_of(shared)
            {
                let _ = ReadableStream::close_requested(ctx, &readable);
            }
            error
        }
    };
    if let Some((cap, _)) = owned.borrow_mut().finish.as_mut() {
        match error {
            Some(reason) => cap.reject(reason),
            None => cap.fulfill(ctx),
        }
    }
}

fn cancel_finish<'js>(
    ctx: &Ctx<'js>, shared: &Shared<'js>, reason: Value<'js>, from_source: bool,
) -> Result<Promise<'js>> {
    let Some(owned) = shared.upgrade() else {
        return Cap::undefined(ctx);
    };
    if let Some((cap, _)) = owned.borrow().finish.as_ref() {
        return Ok(cap.promise());
    }
    let cap = Cap::new(ctx)?;
    let promise = cap.promise();
    let action = if from_source {
        FinishAction::SourceCancel(reason.clone())
    } else {
        FinishAction::SinkAbort(reason.clone())
    };
    owned.borrow_mut().finish = Some((cap, action));
    let (cancel_fn, transformer) = {
        let borrow = owned.borrow();
        (borrow.cancel_fn.clone(), borrow.transformer.clone())
    };
    let capture_error = from_source
        && writable_of(shared)
            .and_then(|writable| WritableStream::stored_error_for_pipe(&writable))
            .is_none();
    let cancelled = match cancel_fn {
        Some(cancel) => {
            match cancel.call::<_, Value>((This(transformer), reason)) {
                Ok(value) => value,
                Err(error) => Cap::rejected(ctx, thrown(ctx, error))?.into_value(),
            }
        }
        None => Value::new_undefined(ctx.clone()),
    };
    if capture_error {
        owned.borrow_mut().finish_error = writable_of(shared)
            .and_then(|writable| WritableStream::stored_error_for_pipe(&writable));
    }
    clear_algorithms(shared);
    let on_ok = {
        let shared = shared.clone();
        Function::new(ctx.clone(), move |ctx: Ctx<'js>| {
            settle_finish(&ctx, &shared, None);
        })?
    };
    let on_err = {
        let shared = shared.clone();
        Function::new(ctx.clone(), move |ctx: Ctx<'js>, reason: Value<'js>| {
            settle_finish(&ctx, &shared, Some(reason));
        })?
    };
    react(ctx, cancelled, Some(on_ok), Some(on_err))?;
    Ok(promise)
}

fn close_finish<'js>(ctx: &Ctx<'js>, shared: &Shared<'js>) -> Result<Promise<'js>> {
    let Some(owned) = shared.upgrade() else {
        return Cap::undefined(ctx);
    };
    if let Some((cap, _)) = owned.borrow().finish.as_ref() {
        return Ok(cap.promise());
    }
    let cap = Cap::new(ctx)?;
    let promise = cap.promise();
    owned.borrow_mut().finish = Some((cap, FinishAction::SinkClose));
    let (flush_fn, transformer) = {
        let borrow = owned.borrow();
        (borrow.flush_fn.clone(), borrow.transformer.clone())
    };
    let flushed = match (flush_fn, controller_of(shared)) {
        (Some(flush), Some(controller)) => {
            match flush.call::<_, Value>((This(transformer), controller)) {
                Ok(value) => value,
                Err(error) => Cap::rejected(ctx, thrown(ctx, error))?.into_value(),
            }
        }
        _ => Value::new_undefined(ctx.clone()),
    };
    clear_algorithms(shared);
    let on_ok = {
        let shared = shared.clone();
        Function::new(ctx.clone(), move |ctx: Ctx<'js>| {
            settle_finish(&ctx, &shared, None);
        })?
    };
    let on_err = {
        let shared = shared.clone();
        Function::new(ctx.clone(), move |ctx: Ctx<'js>, reason: Value<'js>| {
            settle_finish(&ctx, &shared, Some(reason));
        })?
    };
    react(ctx, flushed, Some(on_ok), Some(on_err))?;
    Ok(promise)
}

/// TransformStreamUnblockWrite.
fn unblock_write<'js>(ctx: &Ctx<'js>, shared: &Shared<'js>) {
    if shared
        .upgrade()
        .is_some_and(|owned| owned.borrow().backpressure)
    {
        set_backpressure(ctx, shared, false);
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
    // Specification step 2 of the sink write algorithm: a stream that started
    // erroring while this write was parked rejects with its stored error
    // rather than running the transformer.
    if let Some(reason) =
        writable_of(shared).and_then(|writable| WritableStream::stored_error_for_pipe(&writable))
    {
        cap.reject(reason);
        return;
    }
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
        if let Some(readable) = readable {
            if let Err(error) = ReadableStream::enqueue(ctx, &readable, chunk) {
                let reason = thrown(ctx, error);
                error_both(ctx, shared, reason.clone());
                return Cap::rejected(ctx, reason);
            }
            let backpressure =
                ReadableStream::desired_size(&readable).is_none_or(|size| size <= 0.0);
            if shared
                .upgrade()
                .is_some_and(|owned| owned.borrow().backpressure != backpressure)
            {
                set_backpressure(ctx, shared, backpressure);
            }
        }
        return Cap::undefined(ctx);
    };
    let outcome = controller.map_or_else(
        || Ok(Value::new_undefined(ctx.clone())),
        |controller| transform.call::<_, Value>((This(transformer), chunk, controller)),
    );
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
        let transformer_object = optional_object(&ctx, transformer.0, "transformer")?;
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
            finish: None,
            finish_error: None,
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
            borrow.started = false;
            borrow.hwm = readable_hwm;
            borrow.size_fn = readable_size;
            borrow.pull_fn = Some(Function::new(ctx.clone(), {
                let shared = shared.clone();
                move |ctx: Ctx<'js>| set_backpressure(&ctx, &shared, false)
            })?);
            borrow.cancel_fn = Some(Function::new(ctx.clone(), {
                let shared = shared.clone();
                move |ctx: Ctx<'js>, reason: Opt<Value<'js>>| -> Result<Promise<'js>> {
                    let reason = reason
                        .0
                        .unwrap_or_else(|| Value::new_undefined(ctx.clone()));
                    cancel_finish(&ctx, &shared, reason, true)
                }
            })?);
        }
        owned.borrow_mut().readable = Rc::downgrade(&readable_inner);
        let readable = ReadableStream::wrap(&ctx, Rc::clone(&readable_inner))?;

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
                move |ctx: Ctx<'js>| -> Result<Promise<'js>> { close_finish(&ctx, &shared) }
            })?,
        )?;
        sink.set(
            "abort",
            Function::new(ctx.clone(), {
                let shared = shared.clone();
                move |ctx: Ctx<'js>, reason: Opt<Value<'js>>| -> Result<Promise<'js>> {
                    let reason = reason
                        .0
                        .unwrap_or_else(|| Value::new_undefined(ctx.clone()));
                    cancel_finish(&ctx, &shared, reason, false)
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
        writable_inner.borrow_mut().started = false;
        let writable = WritableStream::wrap(&ctx, Rc::clone(&writable_inner))?;

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
                let writable = writable_of(&shared);
                if let Some(writable) = &writable {
                    writable.borrow_mut().started = true;
                }
                error_both(&ctx, &shared, reason);
                if let Some(writable) = writable {
                    WritableStream::advance_queue(&ctx, &writable);
                }
            })?
        };
        // The readable half's controller starts already-started, so nothing has
        // kicked its first pull. Do it when the transformer's `start` settles,
        // where the specification's start promise would: a readable strategy
        // with room pulls straight away and lifts the initial backpressure.
        let on_ok = {
            let readable_inner = Rc::clone(&readable_inner);
            let writable_inner = Rc::clone(&writable_inner);
            let pin = Pins::hold(&ctx, ReadableStream::keeper(&readable_inner));
            Function::new(ctx.clone(), move |ctx: Ctx<'js>| {
                Pins::release(&ctx, pin);
                readable_inner.borrow_mut().started = true;
                writable_inner.borrow_mut().started = true;
                WritableStream::advance_queue(&ctx, &writable_inner);
                ReadableStream::pull_if_needed(&ctx, &readable_inner);
            })?
        };
        react(&ctx, started, Some(on_ok), Some(on_err))?;

        Ok(Self { readable, writable })
    }

    #[qjs(prop, rename = PredefinedAtom::SymbolToStringTag, configurable)]
    pub const fn to_string_tag() -> &'static str { "TransformStream" }
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
        let already_errored = ReadableStream::stored_error(&readable).is_some();
        if let Err(error) = ReadableStream::enqueue(&ctx, &readable, chunk) {
            let reason = if already_errored {
                thrown(&ctx, error)
            } else {
                ReadableStream::stored_error(&readable).unwrap_or_else(|| thrown(&ctx, error))
            };
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
        if let Some(readable) = readable_of(&shared) {
            let _ = ReadableStream::close_requested(&ctx, &readable);
        }
        if let Some(writable) = writable_of(&shared) {
            let reason = type_error(&ctx, "the transform stream was terminated");
            WritableStream::start_erroring(&ctx, &writable, reason);
        }
        clear_algorithms(&shared);
        unblock_write(&ctx, &shared);
    }

    #[qjs(prop, rename = PredefinedAtom::SymbolToStringTag, configurable)]
    pub const fn to_string_tag() -> &'static str { "TransformStreamDefaultController" }
}
