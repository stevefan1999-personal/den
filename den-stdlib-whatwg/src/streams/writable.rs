//! `WritableStream`, its default writer and its default controller.
//!
//! The write queue is what makes `writer.ready` a real capacity signal: a
//! chunk's promise settles when that chunk's sink write settles, while `ready`
//! settles when the queue drops back under the high-water mark.

use std::{cell::RefCell, rc::Rc};

use rquickjs::{
    Class, Coerced, Ctx, Exception, Function, JsLifetime, Object, Promise, Result, Value,
    atom::PredefinedAtom,
    class::{Trace, Tracer},
    function::{Constructor, Opt, This},
};

use crate::streams::{
    Cap, Pins, method, native::NativeSink, optional_object, range_error, react,
    readable::extract_strategy, thrown, type_error,
};

pub enum WsState<'js> {
    Writable,
    Closed,
    Erroring(Value<'js>),
    Errored(Value<'js>),
}

pub enum WriteRecord<'js> {
    Chunk(Value<'js>),
    Close,
}

pub struct WriterSlot<'js> {
    id:     u64,
    ready:  Cap<'js>,
    closed: Cap<'js>,
}

impl<'js> Trace<'js> for WriterSlot<'js> {
    fn trace<'a>(&self, tracer: Tracer<'a, 'js>) {
        self.ready.trace(tracer);
        self.closed.trace(tracer);
    }
}

pub struct PendingAbort<'js> {
    cap:              Cap<'js>,
    reason:           Value<'js>,
    already_erroring: bool,
}

impl<'js> Trace<'js> for PendingAbort<'js> {
    fn trace<'a>(&self, tracer: Tracer<'a, 'js>) {
        self.cap.trace(tracer);
        self.reason.trace(tracer);
    }
}

pub struct WritableInner<'js> {
    pub(crate) state:            WsState<'js>,
    pub(crate) backpressure:     bool,
    pub(crate) writer:           Option<WriterSlot<'js>>,
    pub(crate) next_writer:      u64,
    pub(crate) controller:       Option<Class<'js, WritableStreamDefaultController<'js>>>,
    pub(crate) queue:            Vec<(WriteRecord<'js>, f64)>,
    pub(crate) queue_total:      f64,
    pub(crate) hwm:              f64,
    pub(crate) size_fn:          Option<Function<'js>>,
    pub(crate) sink:             Object<'js>,
    pub(crate) write_fn:         Option<Function<'js>>,
    pub(crate) close_fn:         Option<Function<'js>>,
    pub(crate) abort_fn:         Option<Function<'js>>,
    pub(crate) native:           Option<Rc<RefCell<NativeSink<'js>>>>,
    pub(crate) abort_controller: Option<Object<'js>>,
    pub(crate) started:          bool,
    pub(crate) in_flight_write:  Option<Cap<'js>>,
    pub(crate) in_flight_close:  Option<Cap<'js>>,
    pub(crate) close_request:    Option<Cap<'js>>,
    pub(crate) write_requests:   Vec<Cap<'js>>,
    pub(crate) pending_abort:    Option<PendingAbort<'js>>,
    /// See `ReadableInner::roots`.
    pub(crate) roots:            Vec<Value<'js>>,
}

impl<'js> Trace<'js> for WritableInner<'js> {
    fn trace<'a>(&self, tracer: Tracer<'a, 'js>) {
        match &self.state {
            WsState::Erroring(reason) | WsState::Errored(reason) => reason.trace(tracer),
            _ => {}
        }
        if let Some(slot) = self.writer.as_ref() {
            slot.trace(tracer);
        }
        self.controller.trace(tracer);
        for (record, _) in &self.queue {
            if let WriteRecord::Chunk(chunk) = record {
                chunk.trace(tracer);
            }
        }
        self.size_fn.trace(tracer);
        self.sink.trace(tracer);
        self.write_fn.trace(tracer);
        self.close_fn.trace(tracer);
        self.abort_fn.trace(tracer);
        self.abort_controller.trace(tracer);
        self.in_flight_write.trace(tracer);
        self.in_flight_close.trace(tracer);
        self.close_request.trace(tracer);
        for request in &self.write_requests {
            request.trace(tracer);
        }
        if let Some(abort) = self.pending_abort.as_ref() {
            abort.trace(tracer);
        }
        self.roots.trace(tracer);
    }
}

// SAFETY: see the matching impl in `readable`.
unsafe impl<'js> rquickjs::JsLifetime<'js> for WritableInner<'js> {
    type Changed<'to> = WritableInner<'to>;
}

pub type Inner<'js> = Rc<RefCell<WritableInner<'js>>>;

#[derive(JsLifetime)]
#[rquickjs::class]
pub struct WritableStream<'js> {
    pub(crate) inner: Inner<'js>,
    /// See [`ReadableStream`](crate::streams::ReadableStream): the controller
    /// is the record's single tracer and the stream owns one reference to it.
    controller:       Class<'js, WritableStreamDefaultController<'js>>,
}

impl<'js> Trace<'js> for WritableStream<'js> {
    fn trace<'a>(&self, tracer: Tracer<'a, 'js>) { self.controller.trace(tracer); }
}

impl<'js> WritableStream<'js> {
    pub(crate) fn new_inner(ctx: &Ctx<'js>) -> Result<Inner<'js>> {
        Ok(Rc::new(RefCell::new(WritableInner {
            state:            WsState::Writable,
            backpressure:     false,
            writer:           None,
            next_writer:      1,
            controller:       None,
            queue:            Vec::new(),
            queue_total:      0.0,
            hwm:              1.0,
            size_fn:          None,
            sink:             Object::new(ctx.clone())?,
            write_fn:         None,
            close_fn:         None,
            abort_fn:         None,
            native:           None,
            abort_controller: None,
            started:          false,
            in_flight_write:  None,
            in_flight_close:  None,
            close_request:    None,
            write_requests:   Vec::new(),
            pending_abort:    None,
            roots:            Vec::new(),
        })))
    }

    pub(crate) fn is_locked(inner: &Inner<'js>) -> bool { inner.borrow().writer.is_some() }

    pub(crate) fn attach_controller(
        ctx: &Ctx<'js>, inner: &Inner<'js>,
    ) -> Result<Class<'js, WritableStreamDefaultController<'js>>> {
        let controller = Class::instance(ctx.clone(), WritableStreamDefaultController {
            inner: Rc::clone(inner),
        })?;
        inner.borrow_mut().controller = Some(controller.clone());
        Ok(controller)
    }

    /// The controller handle every internal reaction holds to keep the stream
    /// alive for the length of the operation it is waiting on.
    pub(crate) fn keeper(
        inner: &Inner<'js>,
    ) -> Option<Class<'js, WritableStreamDefaultController<'js>>> {
        inner.borrow().controller.clone()
    }

    /// Wrap a record built by Rust in its stream object.
    pub(crate) fn wrap(ctx: &Ctx<'js>, inner: Inner<'js>) -> Result<Class<'js, Self>> {
        let controller = match Self::keeper(&inner) {
            Some(controller) => controller,
            None => Self::attach_controller(ctx, &inner)?,
        };
        Class::instance(ctx.clone(), Self { inner, controller })
    }

    /// The pipe needs the destination's failure state and its capacity signal
    /// without going through a writer object.
    pub(crate) fn stored_error_for_pipe(inner: &Inner<'js>) -> Option<Value<'js>> {
        Self::stored_error(inner)
    }

    pub(crate) fn is_closed_for_pipe(inner: &Inner<'js>) -> bool {
        matches!(inner.borrow().state, WsState::Closed) || Self::close_queued_or_in_flight(inner)
    }

    pub(crate) fn is_fully_closed(inner: &Inner<'js>) -> bool {
        matches!(inner.borrow().state, WsState::Closed)
    }

    pub(crate) fn writer_ready(inner: &Inner<'js>) -> Option<Promise<'js>> {
        inner
            .borrow()
            .writer
            .as_ref()
            .map(|slot| slot.ready.promise())
    }

    pub(crate) fn writer_closed(inner: &Inner<'js>) -> Option<Promise<'js>> {
        inner
            .borrow()
            .writer
            .as_ref()
            .map(|slot| slot.closed.promise())
    }

    /// Set up a stream whose sink is built in Rust: the strategy is already in
    /// `inner`, so only the algorithms and the start step are left.
    pub(crate) fn setup_with_sink(
        ctx: &Ctx<'js>, inner: &Inner<'js>, sink: Object<'js>, hwm: f64,
    ) -> Result<()> {
        {
            let mut borrow = inner.borrow_mut();
            borrow.hwm = hwm;
            borrow.sink = sink.clone();
            borrow.write_fn = sink.get("write").ok();
            borrow.close_fn = sink.get("close").ok();
            borrow.abort_fn = sink.get("abort").ok();
            borrow.started = true;
        }
        let abort_controller = ctx
            .globals()
            .get::<_, Constructor>("AbortController")
            .ok()
            .and_then(|ctor| ctor.construct::<_, Object>(()).ok());
        inner.borrow_mut().abort_controller = abort_controller;
        Self::attach_controller(ctx, inner)?;
        Self::recompute_backpressure(ctx, inner);
        Ok(())
    }

    pub(crate) fn writer_pending(inner: &Inner<'js>) -> (bool, bool) {
        inner
            .borrow()
            .writer
            .as_ref()
            .map_or((false, false), |slot| {
                (slot.ready.is_pending(), slot.closed.is_pending())
            })
    }

    pub(crate) fn writer_is_current(inner: &Inner<'js>, id: u64) -> bool {
        inner
            .borrow()
            .writer
            .as_ref()
            .is_some_and(|slot| slot.id == id)
    }

    fn stored_error(inner: &Inner<'js>) -> Option<Value<'js>> {
        match &inner.borrow().state {
            WsState::Erroring(reason) | WsState::Errored(reason) => Some(reason.clone()),
            _ => None,
        }
    }

    fn close_queued_or_in_flight(inner: &Inner<'js>) -> bool {
        let borrow = inner.borrow();
        borrow.close_request.is_some() || borrow.in_flight_close.is_some()
    }

    fn has_operation_in_flight(inner: &Inner<'js>) -> bool {
        let borrow = inner.borrow();
        borrow.in_flight_write.is_some() || borrow.in_flight_close.is_some()
    }

    fn desired_size(inner: &Inner<'js>) -> Option<f64> {
        let borrow = inner.borrow();
        match borrow.state {
            WsState::Errored(_) | WsState::Erroring(_) => None,
            WsState::Closed => Some(0.0),
            WsState::Writable => Some(std::ops::Sub::sub(borrow.hwm, borrow.queue_total)),
        }
    }

    fn clear_algorithms(inner: &Inner<'js>) {
        let mut borrow = inner.borrow_mut();
        borrow.write_fn = None;
        borrow.close_fn = None;
        borrow.abort_fn = None;
        borrow.size_fn = None;
        borrow.native = None;
    }

    pub(crate) fn acquire_writer(ctx: &Ctx<'js>, inner: &Inner<'js>) -> Result<u64> {
        if Self::is_locked(inner) {
            return Err(Exception::throw_type(ctx, "WritableStream is locked"));
        }
        let mut ready = Cap::new(ctx)?;
        let mut closed = Cap::new(ctx)?;
        let id = {
            let mut borrow = inner.borrow_mut();
            let id = borrow.next_writer;
            borrow.next_writer += 1;
            id
        };
        let backpressure = inner.borrow().backpressure;
        let stored = Self::stored_error(inner);
        let closed_or_in_flight = Self::close_queued_or_in_flight(inner);
        match &inner.borrow().state {
            WsState::Writable => {
                if !closed_or_in_flight && backpressure {
                    // leave ready pending
                } else {
                    ready.fulfill(ctx);
                }
            }
            WsState::Erroring(_) => {
                if let Some(reason) = stored.clone() {
                    ready.reject_handled(ctx, reason);
                }
            }
            WsState::Closed => {
                ready.fulfill(ctx);
                closed.fulfill(ctx);
            }
            WsState::Errored(_) => {
                if let Some(reason) = stored.clone() {
                    ready.reject_handled(ctx, reason.clone());
                    closed.reject_handled(ctx, reason);
                }
            }
        }
        inner.borrow_mut().writer = Some(WriterSlot { id, ready, closed });
        Ok(id)
    }

    pub(crate) fn release_writer(
        ctx: &Ctx<'js>, inner: &Inner<'js>, id: u64,
    ) -> Option<Value<'js>> {
        if !Self::writer_is_current(inner, id) {
            return None;
        }
        let reason = type_error(ctx, "the writer was released");
        Self::ensure_ready_rejected(ctx, inner, reason.clone());
        Self::ensure_closed_rejected(ctx, inner, reason.clone());
        inner.borrow_mut().writer = None;
        Some(reason)
    }

    fn ensure_ready_rejected(ctx: &Ctx<'js>, inner: &Inner<'js>, reason: Value<'js>) {
        let mut borrow = inner.borrow_mut();
        if let Some(slot) = borrow.writer.as_mut() {
            if slot.ready.is_pending() {
                slot.ready.reject_handled(ctx, reason);
            } else {
                let Ok(mut cap) = Cap::new(ctx) else { return };
                cap.reject_handled(ctx, reason);
                slot.ready = cap;
            }
        }
    }

    fn ensure_closed_rejected(ctx: &Ctx<'js>, inner: &Inner<'js>, reason: Value<'js>) {
        let mut borrow = inner.borrow_mut();
        if let Some(slot) = borrow.writer.as_mut() {
            if slot.closed.is_pending() {
                slot.closed.reject_handled(ctx, reason);
            } else {
                let Ok(mut cap) = Cap::new(ctx) else { return };
                cap.reject_handled(ctx, reason);
                slot.closed = cap;
            }
        }
    }

    fn update_backpressure(ctx: &Ctx<'js>, inner: &Inner<'js>, backpressure: bool) {
        let current = inner.borrow().backpressure;
        if current != backpressure {
            let mut borrow = inner.borrow_mut();
            if let Some(slot) = borrow.writer.as_mut() {
                if backpressure {
                    if let Ok(cap) = Cap::new(ctx) {
                        slot.ready = cap;
                    }
                } else {
                    slot.ready.fulfill(ctx);
                }
            }
        }
        inner.borrow_mut().backpressure = backpressure;
    }

    fn recompute_backpressure(ctx: &Ctx<'js>, inner: &Inner<'js>) {
        let backpressure = Self::desired_size(inner).is_none_or(|size| size <= 0.0);
        Self::update_backpressure(ctx, inner, backpressure);
    }

    // ---- erroring --------------------------------------------------------

    pub(crate) fn start_erroring(ctx: &Ctx<'js>, inner: &Inner<'js>, reason: Value<'js>) {
        if !matches!(inner.borrow().state, WsState::Writable) {
            return;
        }
        inner.borrow_mut().state = WsState::Erroring(reason.clone());
        Self::ensure_ready_rejected(ctx, inner, reason);
        if !Self::has_operation_in_flight(inner) && inner.borrow().started {
            Self::finish_erroring(ctx, inner);
        }
    }

    fn finish_erroring(ctx: &Ctx<'js>, inner: &Inner<'js>) {
        let Some(reason) = Self::stored_error(inner) else {
            return;
        };
        {
            let mut borrow = inner.borrow_mut();
            borrow.state = WsState::Errored(reason.clone());
            borrow.queue.clear();
            borrow.queue_total = 0.0;
        }
        let requests = std::mem::take(&mut inner.borrow_mut().write_requests);
        for mut request in requests {
            request.reject(reason.clone());
        }
        let abort = inner.borrow_mut().pending_abort.take();
        let Some(mut abort) = abort else {
            Self::reject_close_and_closed(ctx, inner, reason);
            return;
        };
        if abort.already_erroring {
            abort.cap.reject(reason.clone());
            Self::reject_close_and_closed(ctx, inner, reason);
            return;
        }
        let (abort_fn, sink, native) = {
            let borrow = inner.borrow();
            (
                borrow.abort_fn.clone(),
                borrow.sink.clone(),
                borrow.native.clone(),
            )
        };
        if let Some(native) = native {
            native.borrow_mut().abort(abort.reason.clone());
        }
        let outcome = match abort_fn {
            Some(algorithm) => algorithm.call::<_, Value>((This(sink), abort.reason.clone())),
            None => Ok(Value::new_undefined(ctx.clone())),
        };
        Self::clear_algorithms(inner);
        match outcome {
            Ok(value) => {
                let (resolve, reject) = abort.cap.into_parts();
                // Pinning the controller is what keeps the record's JS values alive
                // for the length of this operation: see the note on
                // `ReadableStreamDefaultController`.
                let pin = Pins::hold(ctx, Self::keeper(inner));
                let settle = |handler: Option<Function<'js>>, rejected: bool| {
                    let inner = Rc::clone(inner);
                    let reason = reason.clone();
                    move |ctx: Ctx<'js>, error: Opt<Value<'js>>| {
                        Pins::release(&ctx, pin);
                        if let Some(handler) = handler.as_ref() {
                            if rejected {
                                let _ = handler.call::<_, ()>((error
                                    .0
                                    .unwrap_or_else(|| Value::new_undefined(ctx.clone())),));
                            } else {
                                let _ = handler.call::<_, ()>(());
                            }
                        }
                        WritableStream::reject_close_and_closed(&ctx, &inner, reason.clone());
                    }
                };
                let on_ok = Function::new(ctx.clone(), settle(resolve, false));
                let on_err = Function::new(ctx.clone(), settle(reject, true));
                if let (Ok(on_ok), Ok(on_err)) = (on_ok, on_err) {
                    let _ = react(ctx, value, Some(on_ok), Some(on_err));
                }
            }
            Err(error) => {
                let thrown = thrown(ctx, error);
                abort.cap.reject(thrown);
                Self::reject_close_and_closed(ctx, inner, reason);
            }
        }
    }

    fn reject_close_and_closed(ctx: &Ctx<'js>, inner: &Inner<'js>, reason: Value<'js>) {
        if let Some(mut request) = inner.borrow_mut().close_request.take() {
            request.reject(reason.clone());
        }
        let mut borrow = inner.borrow_mut();
        if let Some(slot) = borrow.writer.as_mut()
            && slot.closed.is_pending()
        {
            slot.closed.reject_handled(ctx, reason);
        }
    }

    fn deal_with_rejection(ctx: &Ctx<'js>, inner: &Inner<'js>, reason: Value<'js>) {
        if matches!(inner.borrow().state, WsState::Writable) {
            Self::start_erroring(ctx, inner, reason);
            return;
        }
        Self::finish_erroring(ctx, inner);
    }

    // ---- queue -----------------------------------------------------------

    pub(crate) fn controller_write(
        ctx: &Ctx<'js>, inner: &Inner<'js>, chunk: Value<'js>, size: f64,
    ) {
        {
            let mut borrow = inner.borrow_mut();
            borrow.queue.push((WriteRecord::Chunk(chunk), size));
            borrow.queue_total = std::ops::Add::add(borrow.queue_total, size);
        }
        if matches!(inner.borrow().state, WsState::Writable)
            && !Self::close_queued_or_in_flight(inner)
        {
            Self::recompute_backpressure(ctx, inner);
        }
        Self::advance_queue(ctx, inner);
    }

    fn controller_close(ctx: &Ctx<'js>, inner: &Inner<'js>) {
        inner.borrow_mut().queue.push((WriteRecord::Close, 0.0));
        Self::advance_queue(ctx, inner);
    }

    pub(crate) fn advance_queue(ctx: &Ctx<'js>, inner: &Inner<'js>) {
        {
            let borrow = inner.borrow();
            if !borrow.started || borrow.in_flight_write.is_some() {
                return;
            }
            if matches!(borrow.state, WsState::Closed | WsState::Errored(_)) {
                return;
            }
        }
        if matches!(inner.borrow().state, WsState::Erroring(_)) {
            Self::finish_erroring(ctx, inner);
            return;
        }
        let is_close = match inner.borrow().queue.first() {
            None => return,
            Some((WriteRecord::Close, _)) => true,
            Some((WriteRecord::Chunk(_), _)) => false,
        };
        if is_close {
            Self::process_close(ctx, inner);
        } else {
            Self::process_write(ctx, inner);
        }
    }

    fn process_write(ctx: &Ctx<'js>, inner: &Inner<'js>) {
        let chunk = match inner.borrow().queue.first() {
            Some((WriteRecord::Chunk(chunk), _)) => chunk.clone(),
            _ => return,
        };
        {
            let mut borrow = inner.borrow_mut();
            if borrow.write_requests.is_empty() {
                return;
            }
            let request = borrow.write_requests.remove(0);
            borrow.in_flight_write = Some(request);
        }
        let (write_fn, sink, controller, native) = {
            let borrow = inner.borrow();
            (
                borrow.write_fn.clone(),
                borrow.sink.clone(),
                borrow.controller.clone(),
                borrow.native.clone(),
            )
        };
        let outcome = if let Some(native) = native {
            crate::streams::native::drive_write(ctx, inner, &native, chunk)
        } else {
            match (write_fn, controller) {
                (Some(write), Some(controller)) => {
                    write.call::<_, Value>((This(sink), chunk, controller))
                }
                _ => Ok(Value::new_undefined(ctx.clone())),
            }
        };
        match outcome {
            Ok(value) => {
                let pin = Pins::hold(ctx, Self::keeper(inner));
                let on_ok = {
                    let inner = Rc::clone(inner);
                    Function::new(ctx.clone(), move |ctx: Ctx<'js>| {
                        Pins::release(&ctx, pin);
                        WritableStream::write_settled(&ctx, &inner);
                    })
                };
                let on_err = {
                    let inner = Rc::clone(inner);
                    Function::new(ctx.clone(), move |ctx: Ctx<'js>, reason: Value<'js>| {
                        Pins::release(&ctx, pin);
                        WritableStream::write_failed(&ctx, &inner, reason);
                    })
                };
                if let (Ok(on_ok), Ok(on_err)) = (on_ok, on_err) {
                    let _ = react(ctx, value, Some(on_ok), Some(on_err));
                }
            }
            Err(error) => Self::write_failed(ctx, inner, thrown(ctx, error)),
        }
    }

    pub(crate) fn write_settled(ctx: &Ctx<'js>, inner: &Inner<'js>) {
        if let Some(mut request) = inner.borrow_mut().in_flight_write.take() {
            request.fulfill(ctx);
        }
        let erroring = matches!(inner.borrow().state, WsState::Erroring(_));
        {
            let mut borrow = inner.borrow_mut();
            if !borrow.queue.is_empty() {
                let (_, size) = borrow.queue.remove(0);
                borrow.queue_total = std::ops::Sub::sub(borrow.queue_total, size).max(0.0);
            }
        }
        if !erroring && !Self::close_queued_or_in_flight(inner) {
            Self::recompute_backpressure(ctx, inner);
        }
        Self::advance_queue(ctx, inner);
    }

    pub(crate) fn write_failed(ctx: &Ctx<'js>, inner: &Inner<'js>, reason: Value<'js>) {
        if let Some(mut request) = inner.borrow_mut().in_flight_write.take() {
            request.reject(reason.clone());
        }
        if matches!(inner.borrow().state, WsState::Writable) {
            Self::clear_algorithms(inner);
        }
        Self::deal_with_rejection(ctx, inner, reason);
    }

    fn process_close(ctx: &Ctx<'js>, inner: &Inner<'js>) {
        {
            let mut borrow = inner.borrow_mut();
            borrow.in_flight_close = borrow.close_request.take();
            if !borrow.queue.is_empty() {
                borrow.queue.remove(0);
            }
        }
        let (close_fn, sink, native) = {
            let borrow = inner.borrow();
            (
                borrow.close_fn.clone(),
                borrow.sink.clone(),
                borrow.native.clone(),
            )
        };
        let outcome = native.map_or_else(
            || {
                close_fn.map_or_else(
                    || Ok(Value::new_undefined(ctx.clone())),
                    |close| close.call::<_, Value>((This(sink),)),
                )
            },
            |native| crate::streams::native::drive_close(ctx, inner, &native),
        );
        Self::clear_algorithms(inner);
        match outcome {
            Ok(value) => {
                let pin = Pins::hold(ctx, Self::keeper(inner));
                let on_ok = {
                    let inner = Rc::clone(inner);
                    Function::new(ctx.clone(), move |ctx: Ctx<'js>| {
                        Pins::release(&ctx, pin);
                        WritableStream::close_settled(&ctx, &inner);
                    })
                };
                let on_err = {
                    let inner = Rc::clone(inner);
                    Function::new(ctx.clone(), move |ctx: Ctx<'js>, reason: Value<'js>| {
                        Pins::release(&ctx, pin);
                        WritableStream::close_failed(&ctx, &inner, reason);
                    })
                };
                if let (Ok(on_ok), Ok(on_err)) = (on_ok, on_err) {
                    let _ = react(ctx, value, Some(on_ok), Some(on_err));
                }
            }
            Err(error) => Self::close_failed(ctx, inner, thrown(ctx, error)),
        }
    }

    pub(crate) fn close_settled(ctx: &Ctx<'js>, inner: &Inner<'js>) {
        if let Some(mut request) = inner.borrow_mut().in_flight_close.take() {
            request.fulfill(ctx);
        }
        if matches!(inner.borrow().state, WsState::Erroring(_))
            && let Some(mut abort) = inner.borrow_mut().pending_abort.take()
        {
            abort.cap.fulfill(ctx);
        }
        inner.borrow_mut().state = WsState::Closed;
        let mut borrow = inner.borrow_mut();
        if let Some(slot) = borrow.writer.as_mut() {
            slot.closed.fulfill(ctx);
        }
    }

    pub(crate) fn close_failed(ctx: &Ctx<'js>, inner: &Inner<'js>, reason: Value<'js>) {
        if let Some(mut request) = inner.borrow_mut().in_flight_close.take() {
            request.reject(reason.clone());
        }
        if let Some(mut abort) = inner.borrow_mut().pending_abort.take() {
            abort.cap.reject(reason.clone());
        }
        Self::deal_with_rejection(ctx, inner, reason);
    }

    // ---- stream operations -----------------------------------------------

    pub(crate) fn abort_stream(
        ctx: &Ctx<'js>, inner: &Inner<'js>, reason: Value<'js>,
    ) -> Result<Promise<'js>> {
        if matches!(inner.borrow().state, WsState::Closed | WsState::Errored(_)) {
            return Cap::undefined(ctx);
        }
        let abort_controller = inner.borrow().abort_controller.clone();
        if let Some(controller) = abort_controller
            && let Ok(abort) = controller.get::<_, Function>("abort")
        {
            let _ = abort.call::<_, Value>((This(controller.clone()), reason.clone()));
        }
        if matches!(inner.borrow().state, WsState::Closed | WsState::Errored(_)) {
            return Cap::undefined(ctx);
        }
        if let Some(pending) = inner.borrow().pending_abort.as_ref() {
            return Ok(pending.cap.promise());
        }
        let already_erroring = matches!(inner.borrow().state, WsState::Erroring(_));
        let reason = if already_erroring {
            Value::new_undefined(ctx.clone())
        } else {
            reason
        };
        let cap = Cap::new(ctx)?;
        let promise = cap.promise();
        inner.borrow_mut().pending_abort = Some(PendingAbort {
            cap,
            reason: reason.clone(),
            already_erroring,
        });
        if !already_erroring {
            Self::start_erroring(ctx, inner, reason);
        }
        Ok(promise)
    }

    pub(crate) fn close_stream(ctx: &Ctx<'js>, inner: &Inner<'js>) -> Result<Promise<'js>> {
        if matches!(inner.borrow().state, WsState::Closed | WsState::Errored(_)) {
            return Cap::rejected(
                ctx,
                type_error(ctx, "the stream is already closed or errored"),
            );
        }
        let cap = Cap::new(ctx)?;
        let promise = cap.promise();
        inner.borrow_mut().close_request = Some(cap);
        let backpressure = inner.borrow().backpressure;
        if backpressure && matches!(inner.borrow().state, WsState::Writable) {
            let mut borrow = inner.borrow_mut();
            if let Some(slot) = borrow.writer.as_mut() {
                slot.ready.fulfill(ctx);
            }
        }
        Self::controller_close(ctx, inner);
        Ok(promise)
    }

    pub(crate) fn writer_write(
        ctx: &Ctx<'js>, inner: &Inner<'js>, writer_id: u64, chunk: Value<'js>,
    ) -> Result<Promise<'js>> {
        let size_fn = inner.borrow().size_fn.clone();
        let size = match size_fn {
            Some(size_fn) => {
                match size_fn.call::<_, Coerced<f64>>((chunk.clone(),)) {
                    Ok(size) if size.0.is_finite() && size.0 >= 0.0 => size.0,
                    Ok(_) => {
                        let reason =
                            range_error(ctx, "a chunk size must be a non-negative finite number");
                        Self::error_if_needed(ctx, inner, reason.clone());
                        return Cap::rejected(ctx, reason);
                    }
                    Err(error) => {
                        let reason = thrown(ctx, error);
                        Self::error_if_needed(ctx, inner, reason.clone());
                        return Cap::rejected(ctx, reason);
                    }
                }
            }
            None => 1.0,
        };
        if !Self::writer_is_current(inner, writer_id) {
            return Cap::rejected(ctx, type_error(ctx, "the writer was released"));
        }
        if let Some(reason) = Self::stored_error(inner)
            && matches!(inner.borrow().state, WsState::Errored(_))
        {
            return Cap::rejected(ctx, reason);
        }
        if Self::close_queued_or_in_flight(inner) || matches!(inner.borrow().state, WsState::Closed)
        {
            return Cap::rejected(
                ctx,
                type_error(ctx, "the stream is closing or already closed"),
            );
        }
        if let Some(reason) = Self::stored_error(inner) {
            return Cap::rejected(ctx, reason);
        }
        let cap = Cap::new(ctx)?;
        let promise = cap.promise();
        inner.borrow_mut().write_requests.push(cap);
        Self::controller_write(ctx, inner, chunk, size);
        Ok(promise)
    }

    fn error_if_needed(ctx: &Ctx<'js>, inner: &Inner<'js>, reason: Value<'js>) {
        if matches!(inner.borrow().state, WsState::Writable) {
            Self::clear_algorithms(inner);
            Self::start_erroring(ctx, inner, reason);
        }
    }

    pub(crate) fn setup(
        ctx: &Ctx<'js>, inner: &Inner<'js>, sink: Object<'js>, strategy: Option<Value<'js>>,
    ) -> Result<()> {
        let (hwm, size_fn) = extract_strategy(ctx, strategy, 1.0)?;
        let start_fn = method(ctx, &sink, "start")?;
        let write_fn = method(ctx, &sink, "write")?;
        let close_fn = method(ctx, &sink, "close")?;
        let abort_fn = method(ctx, &sink, "abort")?;
        let abort_controller = ctx
            .globals()
            .get::<_, Constructor>("AbortController")
            .ok()
            .and_then(|ctor| ctor.construct::<_, Object>(()).ok());
        {
            let mut borrow = inner.borrow_mut();
            borrow.hwm = hwm;
            borrow.size_fn = size_fn;
            borrow.sink = sink.clone();
            borrow.write_fn = write_fn;
            borrow.close_fn = close_fn;
            borrow.abort_fn = abort_fn;
            borrow.abort_controller = abort_controller;
        }
        Self::recompute_backpressure(ctx, inner);
        let controller = Self::attach_controller(ctx, inner)?;
        let started = match start_fn {
            Some(start) => start.call::<_, Value>((This(sink), controller.clone()))?,
            None => Value::new_undefined(ctx.clone()),
        };
        let pin = Pins::hold(ctx, Some(controller.clone()));
        let on_ok = {
            let inner = Rc::clone(inner);
            Function::new(ctx.clone(), move |ctx: Ctx<'js>| {
                Pins::release(&ctx, pin);
                inner.borrow_mut().started = true;
                WritableStream::advance_queue(&ctx, &inner);
            })?
        };
        let on_err = {
            let inner = Rc::clone(inner);
            Function::new(ctx.clone(), move |ctx: Ctx<'js>, reason: Value<'js>| {
                Pins::release(&ctx, pin);
                inner.borrow_mut().started = true;
                WritableStream::deal_with_rejection(&ctx, &inner, reason);
            })?
        };
        react(ctx, started, Some(on_ok), Some(on_err))?;
        Ok(())
    }
}

#[rquickjs::methods(rename_all = "camelCase")]
impl<'js> WritableStream<'js> {
    #[qjs(constructor)]
    pub fn new(ctx: Ctx<'js>, sink: Opt<Value<'js>>, strategy: Opt<Value<'js>>) -> Result<Self> {
        let sink_object = optional_object(&ctx, sink.0, "underlyingSink")?;
        let kind: Value = sink_object.get("type")?;
        if !kind.is_undefined() {
            return Err(Exception::throw_range(
                &ctx,
                "den: underlyingSink.type is reserved and must be undefined",
            ));
        }
        let inner = Self::new_inner(&ctx)?;
        Self::setup(&ctx, &inner, sink_object, strategy.0)?;
        let controller = Self::keeper(&inner)
            .ok_or_else(|| Exception::throw_type(&ctx, "the stream has no controller"))?;
        Ok(Self { inner, controller })
    }

    #[qjs(get)]
    pub fn locked(&self) -> bool { Self::is_locked(&self.inner) }

    pub fn abort(&self, ctx: Ctx<'js>, reason: Opt<Value<'js>>) -> Result<Promise<'js>> {
        if Self::is_locked(&self.inner) {
            return Cap::rejected(&ctx, type_error(&ctx, "WritableStream is locked"));
        }
        Self::abort_stream(
            &ctx,
            &self.inner,
            reason
                .0
                .unwrap_or_else(|| Value::new_undefined(ctx.clone())),
        )
    }

    pub fn close(&self, ctx: Ctx<'js>) -> Result<Promise<'js>> {
        if Self::is_locked(&self.inner) {
            return Cap::rejected(&ctx, type_error(&ctx, "WritableStream is locked"));
        }
        if Self::close_queued_or_in_flight(&self.inner) {
            return Cap::rejected(&ctx, type_error(&ctx, "the stream is already closing"));
        }
        Self::close_stream(&ctx, &self.inner)
    }

    pub fn get_writer(
        this: This<Class<'js, Self>>, ctx: Ctx<'js>,
    ) -> Result<Class<'js, WritableStreamDefaultWriter<'js>>> {
        WritableStreamDefaultWriter::acquire(&ctx, this.0)
    }

    #[qjs(prop, rename = PredefinedAtom::SymbolToStringTag, configurable)]
    pub const fn to_string_tag() -> &'static str { "WritableStream" }
}

/// Owner and single tracer of the record: see the note on
/// [`ReadableStreamDefaultController`](crate::streams::ReadableStreamDefaultController).
#[rquickjs::class]
pub struct WritableStreamDefaultController<'js> {
    inner: Inner<'js>,
}

// SAFETY: the record handle is `'js`-scoped, exactly as the derive would
// generate.
unsafe impl<'js> rquickjs::JsLifetime<'js> for WritableStreamDefaultController<'js> {
    type Changed<'to> = WritableStreamDefaultController<'to>;
}

impl<'js> Trace<'js> for WritableStreamDefaultController<'js> {
    fn trace<'a>(&self, tracer: Tracer<'a, 'js>) {
        if let Ok(inner) = self.inner.try_borrow() {
            inner.trace(tracer);
        }
    }
}

#[rquickjs::methods(rename_all = "camelCase")]
impl<'js> WritableStreamDefaultController<'js> {
    #[qjs(constructor)]
    pub fn new(ctx: Ctx<'js>) -> Result<Self> {
        Err(Exception::throw_type(&ctx, "Illegal constructor"))
    }

    #[qjs(get)]
    pub fn signal(&self, ctx: Ctx<'js>) -> Result<Value<'js>> {
        self.inner.borrow().abort_controller.clone().map_or_else(
            || Ok(Value::new_undefined(ctx)),
            |controller| controller.get("signal"),
        )
    }

    #[qjs(get)]
    pub fn abort_reason(&self, ctx: Ctx<'js>) -> Result<Value<'js>> {
        let signal: Value = self.signal(ctx.clone())?;
        signal.as_object().map_or_else(
            || Ok(Value::new_undefined(ctx)),
            |object| object.get("reason"),
        )
    }

    pub fn error(&self, ctx: Ctx<'js>, reason: Opt<Value<'js>>) {
        let inner = &self.inner;
        if matches!(inner.borrow().state, WsState::Writable) {
            WritableStream::clear_algorithms(inner);
            WritableStream::start_erroring(
                &ctx,
                inner,
                reason
                    .0
                    .unwrap_or_else(|| Value::new_undefined(ctx.clone())),
            );
        }
    }

    #[qjs(prop, rename = PredefinedAtom::SymbolToStringTag, configurable)]
    pub const fn to_string_tag() -> &'static str { "WritableStreamDefaultController" }
}

#[derive(JsLifetime)]
#[rquickjs::class]
pub struct WritableStreamDefaultWriter<'js> {
    stream: Class<'js, WritableStream<'js>>,
    id:     u64,
    ready:  RefCell<Promise<'js>>,
    closed: RefCell<Promise<'js>>,
}

impl<'js> Trace<'js> for WritableStreamDefaultWriter<'js> {
    fn trace<'a>(&self, tracer: Tracer<'a, 'js>) {
        self.stream.trace(tracer);
        if let Ok(ready) = self.ready.try_borrow() {
            ready.trace(tracer);
        }
        if let Ok(closed) = self.closed.try_borrow() {
            closed.trace(tracer);
        }
    }
}

impl<'js> WritableStreamDefaultWriter<'js> {
    pub(crate) fn acquire(
        ctx: &Ctx<'js>, stream: Class<'js, WritableStream<'js>>,
    ) -> Result<Class<'js, Self>> {
        let inner = stream.borrow().inner.clone();
        let id = WritableStream::acquire_writer(ctx, &inner)?;
        let (ready, closed) = {
            let borrow = inner.borrow();
            let slot = borrow
                .writer
                .as_ref()
                .ok_or_else(|| Exception::throw_type(ctx, "WritableStream is locked"))?;
            (slot.ready.promise(), slot.closed.promise())
        };
        Class::instance(ctx.clone(), Self {
            stream,
            id,
            ready: RefCell::new(ready),
            closed: RefCell::new(closed),
        })
    }

    fn inner(&self, ctx: &Ctx<'js>) -> Result<Inner<'js>> {
        let inner = self.stream.borrow().inner.clone();
        if !WritableStream::writer_is_current(&inner, self.id) {
            return Err(Exception::throw_type(ctx, "the writer was released"));
        }
        Ok(inner)
    }
}

#[rquickjs::methods(rename_all = "camelCase")]
impl<'js> WritableStreamDefaultWriter<'js> {
    #[qjs(constructor)]
    pub fn new(ctx: Ctx<'js>, stream: Value<'js>) -> Result<Self> {
        let stream = stream
            .as_object()
            .and_then(Class::<WritableStream>::from_object)
            .ok_or_else(|| Exception::throw_type(&ctx, "a WritableStream is required"))?;
        let inner = stream.borrow().inner.clone();
        let id = WritableStream::acquire_writer(&ctx, &inner)?;
        let (ready, closed) = {
            let borrow = inner.borrow();
            let slot = borrow
                .writer
                .as_ref()
                .ok_or_else(|| Exception::throw_type(&ctx, "WritableStream is locked"))?;
            (slot.ready.promise(), slot.closed.promise())
        };
        Ok(Self {
            stream,
            id,
            ready: RefCell::new(ready),
            closed: RefCell::new(closed),
        })
    }

    #[qjs(get)]
    pub fn ready(&self, ctx: Ctx<'js>) -> Promise<'js> {
        let inner = self.stream.borrow().inner.clone();
        if WritableStream::writer_is_current(&inner, self.id)
            && let Some(slot) = inner.borrow().writer.as_ref()
        {
            let promise = slot.ready.promise();
            *self.ready.borrow_mut() = promise.clone();
            return promise;
        }
        let _ = ctx;
        self.ready.borrow().clone()
    }

    #[qjs(get)]
    pub fn closed(&self, ctx: Ctx<'js>) -> Promise<'js> {
        let inner = self.stream.borrow().inner.clone();
        if WritableStream::writer_is_current(&inner, self.id)
            && let Some(slot) = inner.borrow().writer.as_ref()
        {
            let promise = slot.closed.promise();
            *self.closed.borrow_mut() = promise.clone();
            return promise;
        }
        let _ = ctx;
        self.closed.borrow().clone()
    }

    #[qjs(get)]
    pub fn desired_size(&self, ctx: Ctx<'js>) -> Result<Value<'js>> {
        let inner = self.inner(&ctx)?;
        Ok(match WritableStream::desired_size(&inner) {
            Some(size) => Value::new_float(ctx, size),
            None => Value::new_null(ctx),
        })
    }

    pub fn write(&self, ctx: Ctx<'js>, chunk: Opt<Value<'js>>) -> Result<Promise<'js>> {
        let inner = match self.inner(&ctx) {
            Ok(inner) => inner,
            Err(error) => return Cap::rejected(&ctx, thrown(&ctx, error)),
        };
        WritableStream::writer_write(
            &ctx,
            &inner,
            self.id,
            chunk.0.unwrap_or_else(|| Value::new_undefined(ctx.clone())),
        )
    }

    pub fn close(&self, ctx: Ctx<'js>) -> Result<Promise<'js>> {
        let inner = match self.inner(&ctx) {
            Ok(inner) => inner,
            Err(error) => return Cap::rejected(&ctx, thrown(&ctx, error)),
        };
        if WritableStream::close_queued_or_in_flight(&inner) {
            return Cap::rejected(&ctx, type_error(&ctx, "the stream is already closing"));
        }
        WritableStream::close_stream(&ctx, &inner)
    }

    pub fn abort(&self, ctx: Ctx<'js>, reason: Opt<Value<'js>>) -> Result<Promise<'js>> {
        let inner = match self.inner(&ctx) {
            Ok(inner) => inner,
            Err(error) => return Cap::rejected(&ctx, thrown(&ctx, error)),
        };
        WritableStream::abort_stream(
            &ctx,
            &inner,
            reason
                .0
                .unwrap_or_else(|| Value::new_undefined(ctx.clone())),
        )
    }

    pub fn release_lock(&self, ctx: Ctx<'js>) {
        let inner = self.stream.borrow().inner.clone();
        if !WritableStream::writer_is_current(&inner, self.id) {
            return;
        }
        let (ready_pending, closed_pending) = WritableStream::writer_pending(&inner);
        let _ = self.ready(ctx.clone());
        let _ = self.closed(ctx.clone());
        let Some(reason) = WritableStream::release_writer(&ctx, &inner, self.id) else {
            return;
        };
        if !ready_pending && let Ok(replacement) = Cap::rejected(&ctx, reason.clone()) {
            crate::streams::mark_handled(&ctx, &replacement);
            *self.ready.borrow_mut() = replacement;
        }
        if !closed_pending && let Ok(replacement) = Cap::rejected(&ctx, reason) {
            crate::streams::mark_handled(&ctx, &replacement);
            *self.closed.borrow_mut() = replacement;
        }
    }

    #[qjs(prop, rename = PredefinedAtom::SymbolToStringTag, configurable)]
    pub const fn to_string_tag() -> &'static str { "WritableStreamDefaultWriter" }
}
