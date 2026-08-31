//! Piping, teeing and async iteration.
//!
//! All three drive the source through `ReadRequest::Native`, the
//! specification's read-request record, so none of them creates a
//! `{ value, done }` object or reaches a user-visible `then`.

use std::{
    cell::{Cell, RefCell},
    rc::{Rc, Weak},
};

use rquickjs::{
    Class, Coerced, Ctx, Exception, Function, JsLifetime, Object, Promise, Result, Value,
    atom::PredefinedAtom,
    class::{Trace, Tracer},
    function::{Opt, This},
};

use crate::streams::{
    Cap, chain, mark_handled, react,
    readable::{Inner as RsInner, ReadOutcome, ReadRequest, ReadableStream, iter_result},
    thrown, type_error,
    writable::{Inner as WsInner, WritableStream},
};

/// The pipe holds both streams alive, as the specification's reader and writer
/// do. It is owned and traced by a single [`PipeRecord`] rooted on the source,
/// so the resulting cycle is one QuickJS can see and collect.
struct PipeState<'js> {
    source:          Option<Class<'js, ReadableStream<'js>>>,
    dest:            Option<Class<'js, WritableStream<'js>>>,
    reader_id:       u64,
    writer_id:       u64,
    prevent_close:   bool,
    prevent_abort:   bool,
    prevent_cancel:  bool,
    cap:             Cap<'js>,
    shutting_down:   bool,
    last_write:      Option<Promise<'js>>,
    pending_chunk:   Option<Value<'js>>,
    pending_end:     Option<(Option<Value<'js>>, Shutdown)>,
    pending_actions: usize,
    action_failure:  Option<Value<'js>>,
    action_fallback: Option<Value<'js>>,
    signal:          Option<(Object<'js>, Function<'js>)>,
}

impl<'js> PipeState<'js> {
    fn source_inner(&self) -> Option<RsInner<'js>> {
        Some(self.source.as_ref()?.borrow().inner.clone())
    }

    fn dest_inner(&self) -> Option<WsInner<'js>> {
        Some(self.dest.as_ref()?.borrow().inner.clone())
    }
}

impl<'js> Trace<'js> for PipeState<'js> {
    fn trace<'a>(&self, tracer: Tracer<'a, 'js>) {
        // Traced like any other field: one handle is one reference and one
        // mark. Leaving it out made the pipe's own promise an external root,
        // which kept a pipe that never finishes alive past teardown.
        self.cap.trace(tracer);
        self.source.trace(tracer);
        self.dest.trace(tracer);
        self.last_write.trace(tracer);
        self.pending_chunk.trace(tracer);
        if let Some((Some(reason), _)) = &self.pending_end {
            reason.trace(tracer);
        }
        self.action_failure.trace(tracer);
        self.action_fallback.trace(tracer);
        if let Some((signal, listener)) = &self.signal {
            signal.trace(tracer);
            listener.trace(tracer);
        }
    }
}

/// The single owner and tracer of a running pipe.
#[rquickjs::class]
pub struct PipeRecord<'js> {
    state: Rc<RefCell<PipeState<'js>>>,
}

// SAFETY: every field is a `'js` handle.
unsafe impl<'js> rquickjs::JsLifetime<'js> for PipeRecord<'js> {
    type Changed<'to> = PipeRecord<'to>;
}

impl<'js> Trace<'js> for PipeRecord<'js> {
    fn trace<'a>(&self, tracer: Tracer<'a, 'js>) {
        if let Ok(state) = self.state.try_borrow() {
            state.trace(tracer);
        }
    }
}

type Pipe<'js> = Rc<RefCell<PipeState<'js>>>;

/// What to do to the other end before the pipe promise settles.
#[derive(Clone, Copy)]
enum Shutdown {
    CloseDest,
    AbortDest,
    CancelSource,
    Both,
}

impl<'js> PipeRecord<'js> {
    /// WebIDL boolean conversion: ToBoolean, so `NaN` and `""` are false and
    /// every other object is true.
    fn truthy(object: &Object<'_>, name: &str) -> Result<bool> {
        Ok(object.get::<_, Coerced<bool>>(name)?.0)
    }

    pub(crate) fn pipe_to(
        ctx: &Ctx<'js>, source: &Class<'js, ReadableStream<'js>>, dest: Option<Value<'js>>,
        options: Option<Value<'js>>,
    ) -> Result<Promise<'js>> {
        let dest = dest
            .as_ref()
            .and_then(Value::as_object)
            .and_then(Class::<WritableStream>::from_object)
            .ok_or_else(|| Exception::throw_type(ctx, "pipeTo requires a WritableStream"))?;
        let (prevent_close, prevent_abort, prevent_cancel, signal) =
            PipeRecord::read_pipe_options(ctx, options)?;

        let source_inner = source.borrow().inner.clone();
        let dest_inner = dest.borrow().inner.clone();
        if ReadableStream::is_locked(&source_inner) {
            return Err(Exception::throw_type(ctx, "ReadableStream is locked"));
        }
        if WritableStream::is_locked(&dest_inner) {
            return Err(Exception::throw_type(ctx, "WritableStream is locked"));
        }
        let reader_id = ReadableStream::acquire_reader(ctx, &source_inner)?;
        let writer_id = WritableStream::acquire_writer(ctx, &dest_inner)?;
        source_inner.borrow_mut().disturbed = true;

        let cap = Cap::new(ctx)?;
        let promise = cap.promise();
        let state: Pipe<'js> = Rc::new(RefCell::new(PipeState {
            source: Some(source.clone()),
            dest: Some(dest),
            reader_id,
            writer_id,
            prevent_close,
            prevent_abort,
            prevent_cancel,
            cap,
            shutting_down: false,
            last_write: None,
            pending_chunk: None,
            pending_end: None,
            pending_actions: 0,
            action_failure: None,
            action_fallback: None,
            signal: None,
        }));

        let record = Class::instance(ctx.clone(), PipeRecord {
            state: Rc::clone(&state),
        })?;
        source_inner
            .borrow_mut()
            .roots
            .push(record.clone().into_value());
        dest_inner
            .borrow_mut()
            .roots
            .push(record.clone().into_value());

        if let Some(signal) = signal {
            let aborted: bool = signal.get("aborted").unwrap_or(false);
            if aborted {
                let reason: Value = signal
                    .get("reason")
                    .unwrap_or_else(|_| type_error(ctx, "the pipe was aborted"));
                state.borrow_mut().pending_end = Some((Some(reason), Shutdown::Both));
                let run = Function::new(
                    ctx.clone(),
                    move |ctx: Ctx<'js>, record: Class<'js, PipeRecord<'js>>| {
                        let state = record.borrow().state.clone();
                        let pending = state.borrow_mut().pending_end.take();
                        if let Some((reason, Shutdown::Both)) = pending {
                            PipeRecord::abort_pipe(
                                &ctx,
                                &state,
                                reason.unwrap_or_else(|| type_error(&ctx, "the pipe was aborted")),
                            );
                        }
                    },
                )?;
                let bind: Function = run.get("bind")?;
                let run: Function =
                    bind.call((This(run), Value::new_undefined(ctx.clone()), record.clone()))?;
                run.defer(())?;
                return Ok(promise);
            }
            let listener = {
                let state = Rc::downgrade(&state);
                // The closure captures no JS value: a `RustFunction` traces
                // nothing, so a captured signal would close the cycle
                // signal -> listener -> signal over an edge the collector cannot
                // follow, and every one of those objects is still standing when
                // `JS_FreeRuntime` asserts the heap is empty. The signal is read
                // back out of the pipe's own traced slot instead.
                Function::new(ctx.clone(), move |ctx: Ctx<'js>| {
                    let Some(state) = state.upgrade() else {
                        return;
                    };
                    let signal = state
                        .borrow()
                        .signal
                        .as_ref()
                        .map(|(signal, _)| signal.clone());
                    let reason = signal
                        .and_then(|signal| signal.get::<_, Value>("reason").ok())
                        .unwrap_or_else(|| type_error(&ctx, "the pipe was aborted"));
                    PipeRecord::abort_pipe(&ctx, &state, reason);
                })?
            };
            if let Ok(add) = signal.get::<_, Function>("addEventListener") {
                let _ = add.call::<_, Value>((This(signal.clone()), "abort", listener.clone()));
            }
            state.borrow_mut().signal = Some((signal, listener));
        }

        // Root the pipe on both ends. A running pipe must survive as long as
        // either stream can be observed, and rooting the destination too is what
        // keeps `pipeThrough` alive when script holds only the readable side.
        let _ = record;
        PipeRecord::watch_endpoints(ctx, &state);
        let source_error = ReadableStream::stored_error(&source_inner);
        let dest_error = WritableStream::stored_error_for_pipe(&dest_inner);
        if let Some(reason) = source_error {
            PipeRecord::shutdown(ctx, &state, Some(reason), Shutdown::AbortDest);
        } else if let Some(reason) = dest_error {
            PipeRecord::shutdown(ctx, &state, Some(reason), Shutdown::CancelSource);
        } else if ReadableStream::is_closed(&source_inner) {
            if WritableStream::is_fully_closed(&dest_inner) {
                state.borrow_mut().shutting_down = true;
                PipeRecord::finalize(ctx, &state, None);
            } else {
                PipeRecord::shutdown(ctx, &state, None, Shutdown::CloseDest);
            }
        } else {
            PipeRecord::step(ctx, &state);
        }
        Ok(promise)
    }

    /// Endpoint state changes are independent of backpressure. In particular, a
    /// destination with HWM 0 never lets the read loop run, but source
    /// close/error must still settle the pipe.
    fn watch_endpoints(ctx: &Ctx<'js>, state: &Pipe<'js>) {
        let source_closed = state
            .borrow()
            .source_inner()
            .and_then(|inner| ReadableStream::closed_promise(ctx, &inner).ok());
        if let Some(source_closed) = source_closed {
            let closed = {
                let state = Rc::downgrade(state);
                Function::new(ctx.clone(), move |ctx: Ctx<'js>| {
                    if let Some(state) = state.upgrade() {
                        PipeRecord::shutdown(&ctx, &state, None, Shutdown::CloseDest);
                    }
                })
            };
            let errored = {
                let state = Rc::downgrade(state);
                Function::new(ctx.clone(), move |ctx: Ctx<'js>, reason: Value<'js>| {
                    if let Some(state) = state.upgrade() {
                        PipeRecord::shutdown(&ctx, &state, Some(reason), Shutdown::AbortDest);
                    }
                })
            };
            if let (Ok(closed), Ok(errored)) = (closed, errored) {
                let _ = react(ctx, source_closed.into_value(), Some(closed), Some(errored));
            }
        }

        let dest_closed = state
            .borrow()
            .dest_inner()
            .and_then(|inner| WritableStream::writer_closed(&inner));
        if let Some(dest_closed) = dest_closed {
            let errored = {
                let state = Rc::downgrade(state);
                Function::new(ctx.clone(), move |ctx: Ctx<'js>, reason: Value<'js>| {
                    if let Some(state) = state.upgrade() {
                        PipeRecord::shutdown(&ctx, &state, Some(reason), Shutdown::CancelSource);
                    }
                })
            };
            if let Ok(errored) = errored {
                let _ = react(ctx, dest_closed.into_value(), None, Some(errored));
            }
        }
    }

    fn read_pipe_options(
        ctx: &Ctx<'js>, options: Option<Value<'js>>,
    ) -> Result<(bool, bool, bool, Option<Object<'js>>)> {
        let Some(object) = options
            .filter(|value| !value.is_undefined() && !value.is_null())
            .and_then(rquickjs::Value::into_object)
        else {
            return Ok((false, false, false, None));
        };
        let prevent_abort = PipeRecord::truthy(&object, "preventAbort")?;
        let prevent_cancel = PipeRecord::truthy(&object, "preventCancel")?;
        let prevent_close = PipeRecord::truthy(&object, "preventClose")?;
        let signal: Value = object.get("signal")?;
        let signal = if signal.is_undefined() {
            None
        } else {
            let signal = signal
                .into_object()
                .filter(|object| {
                    Class::<den_stdlib_worker::abort::AbortSignal>::from_object(object).is_some()
                })
                .ok_or_else(|| Exception::throw_type(ctx, "signal must be an AbortSignal"))?;
            Some(signal)
        };
        Ok((prevent_close, prevent_abort, prevent_cancel, signal))
    }

    fn step(ctx: &Ctx<'js>, state: &Pipe<'js>) {
        if state.borrow().shutting_down {
            return;
        }
        let Some(dest_inner) = state.borrow().dest_inner() else {
            return;
        };
        // A destination that already failed or finished ends the pipe before the
        // next read, so a chunk is never read for a sink that cannot take it.
        if let Some(reason) = WritableStream::stored_error_for_pipe(&dest_inner) {
            PipeRecord::shutdown(ctx, state, Some(reason), Shutdown::CancelSource);
            return;
        }
        if WritableStream::is_closed_for_pipe(&dest_inner) {
            let reason = type_error(ctx, "the destination stream is closed");
            PipeRecord::shutdown(ctx, state, Some(reason), Shutdown::CancelSource);
            return;
        }
        let ready = WritableStream::writer_ready(&dest_inner);
        let on_ok = {
            let state = Rc::downgrade(state);
            Function::new(ctx.clone(), move |ctx: Ctx<'js>| {
                if let Some(state) = state.upgrade() {
                    PipeRecord::pump(&ctx, &state);
                }
            })
        };
        let on_err = {
            let state = Rc::downgrade(state);
            Function::new(ctx.clone(), move |ctx: Ctx<'js>, reason: Value<'js>| {
                if let Some(state) = state.upgrade() {
                    PipeRecord::shutdown(&ctx, &state, Some(reason), Shutdown::CancelSource);
                }
            })
        };
        match (ready, on_ok, on_err) {
            (Some(ready), Ok(on_ok), Ok(on_err)) => {
                let _ = react(ctx, ready.into_value(), Some(on_ok), Some(on_err));
            }
            _ => PipeRecord::pump(ctx, state),
        }
    }

    fn pump(ctx: &Ctx<'js>, state: &Pipe<'js>) {
        if state.borrow().shutting_down {
            return;
        }
        let Some(source_inner) = state.borrow().source_inner() else {
            return;
        };
        let request = {
            let state = Rc::downgrade(state);
            ReadRequest::Native(Box::new(
                move |ctx: &Ctx<'js>, outcome: ReadOutcome<'js>| {
                    let Some(state) = state.upgrade() else {
                        return;
                    };
                    match outcome {
                        ReadOutcome::Chunk(chunk) => {
                            state.borrow_mut().pending_chunk = Some(chunk);
                            let pending = {
                                let state = Rc::downgrade(&state);
                                Function::new(ctx.clone(), move |ctx: Ctx<'js>| {
                                    if let Some(state) = state.upgrade() {
                                        PipeRecord::write_chunk(&ctx, &state);
                                    }
                                })
                            };
                            if let Ok(pending) = pending {
                                let _ = pending.defer(());
                            }
                        }
                        ReadOutcome::Close => {
                            PipeRecord::shutdown(ctx, &state, None, Shutdown::CloseDest)
                        }
                        ReadOutcome::Error(reason) => {
                            PipeRecord::shutdown(ctx, &state, Some(reason), Shutdown::AbortDest)
                        }
                    }
                },
            ))
        };
        ReadableStream::read(ctx, &source_inner, request);
    }

    fn write_chunk(ctx: &Ctx<'js>, state: &Pipe<'js>) {
        if state.borrow().shutting_down {
            return;
        }
        let Some(chunk) = state.borrow_mut().pending_chunk.take() else {
            return;
        };
        let Some(dest_inner) = state.borrow().dest_inner() else {
            return;
        };
        let writer_id = state.borrow().writer_id;
        match WritableStream::writer_write(ctx, &dest_inner, writer_id, chunk) {
            Ok(promise) => {
                mark_handled(ctx, &promise);
                state.borrow_mut().last_write = Some(promise);
                let pending = state.borrow_mut().pending_end.take();
                match pending {
                    Some((error, action)) => PipeRecord::shutdown(ctx, state, error, action),
                    None => PipeRecord::step(ctx, state),
                }
            }
            Err(error) => {
                PipeRecord::shutdown(ctx, state, Some(thrown(ctx, error)), Shutdown::CancelSource)
            }
        }
    }

    fn abort_pipe(ctx: &Ctx<'js>, state: &Pipe<'js>, reason: Value<'js>) {
        PipeRecord::shutdown(ctx, state, Some(reason), Shutdown::Both);
    }

    fn shutdown(ctx: &Ctx<'js>, state: &Pipe<'js>, error: Option<Value<'js>>, action: Shutdown) {
        if state.borrow().shutting_down {
            return;
        }
        if state.borrow().pending_chunk.is_some() {
            let mut borrow = state.borrow_mut();
            if matches!(action, Shutdown::Both) || borrow.pending_end.is_none() {
                borrow.pending_end = Some((error, action));
            }
            return;
        }
        state.borrow_mut().shutting_down = true;
        let last_write = state.borrow_mut().last_write.take();
        if let Some(last_write) = last_write {
            let complete = {
                let state = Rc::downgrade(state);
                let error = error.clone();
                Function::new(ctx.clone(), move |ctx: Ctx<'js>| {
                    if let Some(state) = state.upgrade() {
                        PipeRecord::finish_shutdown(&ctx, &state, error.clone(), action);
                    }
                })
            };
            let failed = {
                let state = Rc::downgrade(state);
                Function::new(ctx.clone(), move |ctx: Ctx<'js>, reason: Value<'js>| {
                    if let Some(state) = state.upgrade() {
                        PipeRecord::finish_shutdown(
                            &ctx,
                            &state,
                            Some(reason),
                            Shutdown::CancelSource,
                        );
                    }
                })
            };
            if let (Ok(complete), Ok(failed)) = (complete, failed) {
                let _ = react(ctx, last_write.into_value(), Some(complete), Some(failed));
                return;
            }
        }
        PipeRecord::finish_shutdown(ctx, state, error, action);
    }

    fn finish_shutdown(
        ctx: &Ctx<'js>, state: &Pipe<'js>, error: Option<Value<'js>>, action: Shutdown,
    ) {
        let (prevent_close, prevent_abort, prevent_cancel) = {
            let borrow = state.borrow();
            (
                borrow.prevent_close,
                borrow.prevent_abort,
                borrow.prevent_cancel,
            )
        };
        let dest_inner = state.borrow().dest_inner();
        let source_inner = state.borrow().source_inner();
        let mut pending: Option<Promise<'js>> = None;
        let (Some(dest_inner), Some(source_inner)) = (dest_inner, source_inner) else {
            PipeRecord::finalize(ctx, state, error);
            return;
        };
        match action {
            Shutdown::CloseDest => {
                if !prevent_close {
                    pending = WritableStream::close_stream(ctx, &dest_inner).ok();
                }
            }
            Shutdown::AbortDest => {
                if !prevent_abort && let Some(reason) = error.clone() {
                    pending = WritableStream::abort_stream(ctx, &dest_inner, reason).ok();
                }
            }
            Shutdown::CancelSource => {
                if !prevent_cancel && let Some(reason) = error.clone() {
                    pending = ReadableStream::cancel_stream(ctx, &source_inner, reason).ok();
                }
            }
            Shutdown::Both => {
                let reason = error
                    .clone()
                    .unwrap_or_else(|| type_error(ctx, "the pipe was aborted"));
                let mut actions = Vec::new();
                if !prevent_abort
                    && WritableStream::stored_error_for_pipe(&dest_inner).is_none()
                    && !WritableStream::is_closed_for_pipe(&dest_inner)
                    && let Ok(promise) =
                        WritableStream::abort_stream(ctx, &dest_inner, reason.clone())
                {
                    actions.push((promise, true));
                }
                if !prevent_cancel
                    && ReadableStream::stored_error(&source_inner).is_none()
                    && !ReadableStream::is_closed(&source_inner)
                    && let Ok(promise) = ReadableStream::cancel_stream(ctx, &source_inner, reason)
                {
                    actions.push((promise, false));
                }
                if !actions.is_empty() {
                    let record = source_inner
                        .borrow()
                        .roots
                        .iter()
                        .find_map(|value| Class::<PipeRecord<'js>>::from_value(value).ok());
                    {
                        let mut borrow = state.borrow_mut();
                        borrow.pending_actions = actions.len();
                        borrow.action_fallback = error;
                    }
                    for (action, preferred) in actions {
                        let handlers = record.clone().map(|record| {
                            let on_ok = Function::new(
                                ctx.clone(),
                                move |ctx: Ctx<'js>, record: Class<'js, PipeRecord<'js>>| {
                                    PipeRecord::action_settled(
                                        &ctx,
                                        &record.borrow().state,
                                        None,
                                        false,
                                    );
                                },
                            )?;
                            let bind: Function = on_ok.get("bind")?;
                            let on_ok: Function = bind.call((
                                This(on_ok),
                                Value::new_undefined(ctx.clone()),
                                record.clone(),
                            ))?;
                            let on_err = Function::new(
                                ctx.clone(),
                                move |ctx: Ctx<'js>,
                                      record: Class<'js, PipeRecord<'js>>,
                                      reason: Value<'js>| {
                                    PipeRecord::action_settled(
                                        &ctx,
                                        &record.borrow().state,
                                        Some(reason),
                                        preferred,
                                    );
                                },
                            )?;
                            let bind: Function = on_err.get("bind")?;
                            let on_err = bind.call((
                                This(on_err),
                                Value::new_undefined(ctx.clone()),
                                record,
                            ))?;
                            Ok::<_, rquickjs::Error>((on_ok, on_err))
                        });
                        if let Some(Ok((on_ok, on_err))) = handlers {
                            let _ = react(ctx, action.into_value(), Some(on_ok), Some(on_err));
                        } else {
                            PipeRecord::action_settled(
                                ctx,
                                state,
                                Some(type_error(ctx, "pipe abort action could not be observed")),
                                preferred,
                            );
                        }
                    }
                    return;
                }
            }
        }
        match pending {
            Some(promise) => {
                let finish = |ok: bool| {
                    let state = Rc::downgrade(state);
                    let error = error.clone();
                    move |ctx: Ctx<'js>, reason: Opt<Value<'js>>| {
                        let Some(state) = state.upgrade() else {
                            return;
                        };
                        let failure = if ok {
                            error.clone()
                        } else {
                            Some(
                                reason
                                    .0
                                    .unwrap_or_else(|| Value::new_undefined(ctx.clone())),
                            )
                        };
                        PipeRecord::finalize(&ctx, &state, failure);
                    }
                };
                let on_ok = Function::new(ctx.clone(), finish(true));
                let on_err = Function::new(ctx.clone(), finish(false));
                match (on_ok, on_err) {
                    (Ok(on_ok), Ok(on_err)) => {
                        let _ = react(ctx, promise.into_value(), Some(on_ok), Some(on_err));
                    }
                    _ => PipeRecord::finalize(ctx, state, error),
                }
            }
            None => PipeRecord::finalize(ctx, state, error),
        }
    }

    fn action_settled(
        ctx: &Ctx<'js>, state: &Pipe<'js>, failure: Option<Value<'js>>, preferred: bool,
    ) {
        let done = {
            let mut borrow = state.borrow_mut();
            if preferred || borrow.action_failure.is_none() {
                borrow.action_failure = failure;
            }
            borrow.pending_actions = borrow.pending_actions.saturating_sub(1);
            borrow.pending_actions == 0
        };
        if !done {
            return;
        }
        let fallback = state.borrow_mut().action_fallback.take();
        let failure = state.borrow_mut().action_failure.take().or(fallback);
        PipeRecord::finalize(ctx, state, failure);
    }

    /// Drop this pipe's root from a stream that outlives it. A finished pipe
    /// left in `roots` keeps its result capability — a JS promise no tracer
    /// reaches, because the reactions holding it are `Function::new`
    /// closures — alive for as long as the stream is, which QuickJS reports
    /// as a leak at teardown.
    fn unroot(roots: &mut Vec<Value<'js>>, state: &Pipe<'js>) {
        roots.retain(|value| {
            Class::<PipeRecord<'js>>::from_value(value)
                .ok()
                .is_none_or(|record| !Rc::ptr_eq(&record.borrow().state, state))
        });
    }

    fn finalize(ctx: &Ctx<'js>, state: &Pipe<'js>, error: Option<Value<'js>>) {
        let source_inner = state.borrow().source_inner();
        let dest_inner = state.borrow().dest_inner();
        let reader_id = state.borrow().reader_id;
        let writer_id = state.borrow().writer_id;
        let signal = state.borrow_mut().signal.take();
        // Drop both ends now the pipe is over: the record stays rooted on the
        // streams it served, so holding them would keep the whole graph alive
        // until the streams themselves die.
        state.borrow_mut().source = None;
        state.borrow_mut().dest = None;
        if let Some(inner) = &source_inner {
            PipeRecord::unroot(&mut inner.borrow_mut().roots, state);
        }
        if let Some(inner) = &dest_inner {
            PipeRecord::unroot(&mut inner.borrow_mut().roots, state);
        }
        if let Some((signal, listener)) = signal
            && let Ok(remove) = signal.get::<_, Function>("removeEventListener")
        {
            let _ = remove.call::<_, Value>((This(signal.clone()), "abort", listener));
        }
        if let Some(dest_inner) = dest_inner {
            let _ = WritableStream::release_writer(ctx, &dest_inner, writer_id);
        }
        if let Some(source_inner) = source_inner {
            ReadableStream::release_reader(ctx, &source_inner, reader_id);
        }
        let mut borrow = state.borrow_mut();
        match error {
            Some(reason) => borrow.cap.reject(reason),
            None => borrow.cap.fulfill(ctx),
        }
    }
}

pub fn pipe_through<'js>(
    ctx: &Ctx<'js>, source: &Class<'js, ReadableStream<'js>>, transform: Option<Value<'js>>,
    options: Option<Value<'js>>,
) -> Result<Value<'js>> {
    let pair = transform
        .filter(rquickjs::Value::is_object)
        .and_then(Value::into_object)
        .ok_or_else(|| {
            Exception::throw_type(ctx, "pipeThrough requires a { readable, writable } pair")
        })?;
    let readable: Value = pair.get("readable")?;
    if readable
        .as_object()
        .and_then(Class::<ReadableStream>::from_object)
        .is_none()
    {
        return Err(Exception::throw_type(
            ctx,
            "pipeThrough.readable must be a ReadableStream",
        ));
    }
    let writable: Value = pair.get("writable")?;
    if writable
        .as_object()
        .and_then(Class::<WritableStream>::from_object)
        .is_none()
    {
        return Err(Exception::throw_type(
            ctx,
            "pipeThrough.writable must be a WritableStream",
        ));
    }
    let promise = PipeRecord::pipe_to(ctx, source, Some(writable), options)?;
    mark_handled(ctx, &promise);
    Ok(readable)
}

// ---- tee -----------------------------------------------------------------

/// Owned and traced by a single [`TeeRecord`] rooted on both branches.
struct TeeState<'js> {
    source:      Class<'js, ReadableStream<'js>>,
    reader_id:   u64,
    reading:     bool,
    read_again:  bool,
    canceled:    [bool; 2],
    reasons:     [Option<Value<'js>>; 2],
    branches:    [Weak<RefCell<crate::streams::readable::ReadableInner<'js>>>; 2],
    cancel:      Option<Cap<'js>>,
    clone_chunk: Option<Function<'js>>,
}

impl<'js> Trace<'js> for TeeState<'js> {
    fn trace<'a>(&self, tracer: Tracer<'a, 'js>) {
        if let Some(cap) = self.cancel.as_ref() {
            cap.trace(tracer);
        }
        self.clone_chunk.trace(tracer);
        self.source.trace(tracer);
        for reason in self.reasons.iter().flatten() {
            reason.trace(tracer);
        }
    }
}

/// The single owner and tracer of a tee.
#[rquickjs::class]
pub struct TeeRecord<'js> {
    state: Rc<RefCell<TeeState<'js>>>,
}

// SAFETY: every field is a `'js` handle.
unsafe impl<'js> rquickjs::JsLifetime<'js> for TeeRecord<'js> {
    type Changed<'to> = TeeRecord<'to>;
}

impl<'js> Trace<'js> for TeeRecord<'js> {
    fn trace<'a>(&self, tracer: Tracer<'a, 'js>) {
        if let Ok(state) = self.state.try_borrow() {
            state.trace(tracer);
        }
    }
}

type Tee<'js> = Rc<RefCell<TeeState<'js>>>;

pub fn tee<'js>(
    ctx: &Ctx<'js>, source: &Class<'js, ReadableStream<'js>>, clone_for_branch2: bool,
) -> Result<(
    Class<'js, ReadableStream<'js>>,
    Class<'js, ReadableStream<'js>>,
)> {
    let source_inner = source.borrow().inner.clone();
    let reader_id = ReadableStream::acquire_reader(ctx, &source_inner)?;
    let clone_chunk = clone_for_branch2
        .then(|| {
            ctx.eval::<Function, _>(
                "value => {
                    if (value instanceof ArrayBuffer) return value.slice(0);
                    if (!ArrayBuffer.isView(value)) return value;
                    const buffer = value.buffer.slice(0);
                    return value instanceof DataView
                        ? new DataView(buffer, value.byteOffset, value.byteLength)
                        : new value.constructor(buffer, value.byteOffset, value.length);
                }",
            )
        })
        .transpose()?;
    let state: Tee<'js> = Rc::new(RefCell::new(TeeState {
        source: source.clone(),
        reader_id,
        reading: false,
        read_again: false,
        canceled: [false, false],
        reasons: [None, None],
        branches: [Weak::new(), Weak::new()],
        cancel: None,
        clone_chunk,
    }));
    let left = tee_branch(ctx, &state, 0)?;
    let right = tee_branch(ctx, &state, 1)?;
    let record = Class::instance(ctx.clone(), TeeRecord {
        state: Rc::clone(&state),
    })?;
    for branch in [&left, &right] {
        branch
            .borrow()
            .inner
            .borrow_mut()
            .roots
            .push(record.clone().into_value());
    }
    let closed = ReadableStream::closed_promise(ctx, &source_inner)?;
    let finished = {
        let state = Rc::downgrade(&state);
        Function::new(ctx.clone(), move |ctx: Ctx<'js>| {
            let Some(state) = state.upgrade() else {
                return;
            };
            let (branches, canceled) = {
                let borrow = state.borrow();
                let [left, right] = &borrow.branches;
                ([left.upgrade(), right.upgrade()], borrow.canceled)
            };
            for (branch, canceled) in branches.iter().zip(canceled) {
                if !canceled && let Some(branch) = branch {
                    let _ = ReadableStream::close_requested(&ctx, branch);
                }
            }
            if canceled.iter().any(|canceled| *canceled)
                && let Some(cap) = state.borrow_mut().cancel.as_mut()
            {
                cap.fulfill(&ctx);
            }
        })?
    };
    let errored = {
        let state = Rc::downgrade(&state);
        Function::new(ctx.clone(), move |ctx: Ctx<'js>, reason: Value<'js>| {
            let Some(state) = state.upgrade() else {
                return;
            };
            let branches = {
                let borrow = state.borrow();
                [borrow.branches[0].upgrade(), borrow.branches[1].upgrade()]
            };
            for branch in branches.iter().flatten() {
                ReadableStream::error(&ctx, branch, reason.clone());
            }
            if state.borrow().canceled.iter().any(|canceled| !canceled)
                && let Some(cap) = state.borrow_mut().cancel.as_mut()
            {
                cap.fulfill(&ctx);
            }
        })?
    };
    react(ctx, closed.into_value(), Some(finished), Some(errored))?;
    for branch in [&left, &right] {
        Function::new(
            ctx.clone(),
            move |ctx: Ctx<'js>, branch: Class<'js, ReadableStream<'js>>| {
                ReadableStream::pull_if_needed(&ctx, &branch.borrow().inner);
            },
        )?
        .defer((branch.clone(),))?;
    }
    Ok((left, right))
}

fn tee_branch<'js>(
    ctx: &Ctx<'js>, state: &Tee<'js>, index: usize,
) -> Result<Class<'js, ReadableStream<'js>>> {
    let inner = ReadableStream::new_inner(ctx)?;
    {
        let mut borrow = inner.borrow_mut();
        borrow.started = true;
        borrow.pull_fn = Some(Function::new(ctx.clone(), {
            let state = Rc::downgrade(state);
            move |ctx: Ctx<'js>| {
                if let Some(state) = state.upgrade() {
                    tee_pull(&ctx, &state);
                }
            }
        })?);
        borrow.cancel_fn = Some(Function::new(ctx.clone(), {
            let state = Rc::downgrade(state);
            move |ctx: Ctx<'js>, reason: Opt<Value<'js>>| -> Result<Promise<'js>> {
                let Some(state) = state.upgrade() else {
                    return Cap::undefined(&ctx);
                };
                tee_cancel(
                    &ctx,
                    &state,
                    index,
                    reason
                        .0
                        .unwrap_or_else(|| Value::new_undefined(ctx.clone())),
                )
            }
        })?);
    }
    ReadableStream::attach_controller(ctx, &inner)?;
    let weak = Rc::downgrade(&inner);
    let mut state = state.borrow_mut();
    let Some(branch) = state.branches.get_mut(index) else {
        return Err(Exception::throw_range(
            ctx,
            "tee branch index is out of range",
        ));
    };
    *branch = weak;
    drop(state);
    ReadableStream::wrap(ctx, inner)
}

fn tee_pull<'js>(ctx: &Ctx<'js>, state: &Tee<'js>) {
    if state.borrow().reading {
        state.borrow_mut().read_again = true;
        return;
    }
    state.borrow_mut().reading = true;
    let source_inner = state.borrow().source.borrow().inner.clone();
    let request = {
        let state = Rc::downgrade(state);
        ReadRequest::Native(Box::new(
            move |ctx: &Ctx<'js>, outcome: ReadOutcome<'js>| {
                let Some(state) = state.upgrade() else {
                    return;
                };
                state.borrow_mut().reading = false;
                let branches = {
                    let borrow = state.borrow();
                    let [left, right] = &borrow.branches;
                    [left.upgrade(), right.upgrade()]
                };
                let canceled = state.borrow().canceled;
                match outcome {
                    ReadOutcome::Chunk(chunk) => {
                        for (index, (branch, canceled)) in branches.iter().zip(canceled).enumerate()
                        {
                            if canceled {
                                continue;
                            }
                            if let Some(branch) = branch {
                                let value = if index == 1 {
                                    state
                                        .borrow()
                                        .clone_chunk
                                        .as_ref()
                                        .and_then(|clone| clone.call((chunk.clone(),)).ok())
                                        .unwrap_or_else(|| chunk.clone())
                                } else {
                                    chunk.clone()
                                };
                                let _ = ReadableStream::enqueue(ctx, branch, value);
                            }
                        }
                        let again = std::mem::take(&mut state.borrow_mut().read_again);
                        if again {
                            tee_pull(ctx, &state);
                        }
                    }
                    ReadOutcome::Close => {
                        for (branch, canceled) in branches.iter().zip(canceled) {
                            if canceled {
                                continue;
                            }
                            if let Some(branch) = branch {
                                let _ = ReadableStream::close_requested(ctx, branch);
                            }
                        }
                        if canceled.iter().any(|canceled| *canceled)
                            && let Some(cap) = state.borrow_mut().cancel.as_mut()
                        {
                            cap.fulfill(ctx);
                        }
                    }
                    ReadOutcome::Error(reason) => {
                        for branch in branches.iter().flatten() {
                            ReadableStream::error(ctx, branch, reason.clone());
                        }
                    }
                }
            },
        ))
    };
    ReadableStream::read(ctx, &source_inner, request);
}

fn tee_cancel<'js>(
    ctx: &Ctx<'js>, state: &Tee<'js>, index: usize, reason: Value<'js>,
) -> Result<Promise<'js>> {
    {
        let mut borrow = state.borrow_mut();
        let Some(canceled) = borrow.canceled.get_mut(index) else {
            return Err(Exception::throw_range(
                ctx,
                "tee branch index is out of range",
            ));
        };
        *canceled = true;
        let Some(slot) = borrow.reasons.get_mut(index) else {
            return Err(Exception::throw_range(
                ctx,
                "tee branch index is out of range",
            ));
        };
        *slot = Some(reason);
    }
    if state.borrow().cancel.is_none() {
        state.borrow_mut().cancel = Some(Cap::new(ctx)?);
    }
    let promise = state
        .borrow()
        .cancel
        .as_ref()
        .map(Cap::promise)
        .ok_or_else(|| Exception::throw_type(ctx, "tee cancel is unavailable"))?;
    let both = state.borrow().canceled.iter().all(|canceled| *canceled);
    let source_inner = state.borrow().source.borrow().inner.clone();
    if !both && ReadableStream::is_closed(&source_inner) {
        if let Some(cap) = state.borrow_mut().cancel.as_mut() {
            cap.fulfill(ctx);
        }
        return Ok(promise);
    }
    if both {
        let composite = rquickjs::Array::new(ctx.clone())?;
        for (slot, reason) in state.borrow().reasons.iter().enumerate() {
            composite.set(
                slot,
                reason
                    .clone()
                    .unwrap_or_else(|| Value::new_undefined(ctx.clone())),
            )?;
        }
        let reader_id = state.borrow().reader_id;
        ReadableStream::release_reader(ctx, &source_inner, reader_id);
        let cancelled = ReadableStream::cancel_stream(ctx, &source_inner, composite.into_value())?;
        // Specification step 13: resolve the shared capability *with* the
        // cancel result, so both branches see the same settlement. Fulfilling
        // it unconditionally told the branch that cancelled first that the
        // teardown succeeded even when the source's cancel rejected.
        if let Some(mut cap) = state.borrow_mut().cancel.take() {
            cap.resolve(cancelled.into_value());
        }
        return Ok(promise);
    }
    Ok(promise)
}

// ---- async iteration -----------------------------------------------------

#[derive(JsLifetime)]
#[rquickjs::class]
pub struct ReadableStreamAsyncIterator<'js> {
    stream:         Class<'js, ReadableStream<'js>>,
    reader_id:      u64,
    prevent_cancel: bool,
    finished:       Rc<Cell<bool>>,
    last_next:      RefCell<Option<Promise<'js>>>,
    returning:      RefCell<Option<Promise<'js>>>,
}

impl<'js> Trace<'js> for ReadableStreamAsyncIterator<'js> {
    fn trace<'a>(&self, tracer: Tracer<'a, 'js>) {
        self.stream.trace(tracer);
        if let Ok(returning) = self.returning.try_borrow() {
            returning.trace(tracer);
        }
        if let Ok(last_next) = self.last_next.try_borrow() {
            last_next.trace(tracer);
        }
    }
}

fn after<'js>(ctx: &Ctx<'js>, promise: Promise<'js>, value: Value<'js>) -> Result<Promise<'js>> {
    let cap = Cap::new(ctx)?;
    let mapped = cap.promise();
    let (resolve, reject) = cap.into_parts();
    let Some(resolve) = resolve else {
        return Ok(mapped);
    };
    let bind: Function = resolve.get("bind")?;
    let on_ok: Function = bind.call((This(resolve), Value::new_undefined(ctx.clone()), value))?;
    react(ctx, promise.into_value(), Some(on_ok), reject)?;
    Ok(mapped)
}

pub fn values<'js>(
    ctx: &Ctx<'js>, stream: Class<'js, ReadableStream<'js>>, options: Option<Value<'js>>,
) -> Result<Class<'js, ReadableStreamAsyncIterator<'js>>> {
    if let Some(prototype) = Class::<ReadableStreamAsyncIterator>::prototype(ctx)? {
        let async_iterator_prototype: Object = ctx.eval(
            "Object.getPrototypeOf(Object.getPrototypeOf(async function* () {}).prototype)",
        )?;
        prototype.set_prototype(Some(&async_iterator_prototype))?;
        let expose: Function = ctx.eval(
            "prototype => { for (const [name, length] of [['next', 0], ['return', 1]]) { \
             Object.defineProperty(prototype, name, { enumerable: true }); \
             Object.defineProperties(prototype[name], { name: { value: name, configurable: true \
             }, length: { value: length, configurable: true } }); } }",
        )?;
        expose.call::<_, ()>((prototype,))?;
    }
    let prevent_cancel = match options
        .filter(|value| !value.is_undefined() && !value.is_null())
        .and_then(Value::into_object)
    {
        Some(object) => PipeRecord::truthy(&object, "preventCancel")?,
        None => false,
    };
    let inner = stream.borrow().inner.clone();
    let reader_id = ReadableStream::acquire_reader(ctx, &inner)?;
    Class::instance(ctx.clone(), ReadableStreamAsyncIterator {
        stream,
        reader_id,
        prevent_cancel,
        finished: Rc::new(Cell::new(false)),
        last_next: RefCell::new(None),
        returning: RefCell::new(None),
    })
}

#[rquickjs::methods(rename_all = "camelCase")]
impl<'js> ReadableStreamAsyncIterator<'js> {
    pub fn next(&self, ctx: Ctx<'js>) -> Result<Promise<'js>> {
        let inner = self.stream.borrow().inner.clone();
        if self.finished.get()
            || self.returning.borrow().is_some()
            || !ReadableStream::reader_is_current(&inner, self.reader_id)
        {
            let result = iter_result(&ctx, Value::new_undefined(ctx.clone()), true)?;
            return match self.returning.borrow().clone() {
                Some(returning) => after(&ctx, returning, result),
                None => Cap::resolved(&ctx, result),
            };
        }
        let cap = Cap::new(&ctx)?;
        let promise = cap.promise();
        ReadableStream::read(&ctx, &inner, ReadRequest::AsyncIterator {
            cap,
            finished: Rc::clone(&self.finished),
            inner: Rc::downgrade(&inner),
            reader_id: self.reader_id,
        });
        let promise = chain(&ctx, promise)?;
        *self.last_next.borrow_mut() = Some(promise.clone());
        Ok(promise)
    }

    #[qjs(rename = "return")]
    pub fn iterator_return(&self, ctx: Ctx<'js>, value: Opt<Value<'js>>) -> Result<Promise<'js>> {
        let value = value.0.unwrap_or_else(|| Value::new_undefined(ctx.clone()));
        let inner = self.stream.borrow().inner.clone();
        let result = iter_result(&ctx, value.clone(), true)?;
        if self.finished.get() || self.returning.borrow().is_some() {
            return match self.returning.borrow().clone() {
                Some(returning) => after(&ctx, returning, result),
                None => Cap::resolved(&ctx, result),
            };
        }
        if let Some(previous) = self.last_next.borrow().clone()
            && ReadableStream::reader_is_current(&inner, self.reader_id)
        {
            let released_early = !ReadableStream::has_pending_reads(&inner);
            if released_early {
                ReadableStream::release_reader(&ctx, &inner, self.reader_id);
            }
            let weak = Rc::downgrade(&inner);
            let reader_id = self.reader_id;
            let prevent_cancel = self.prevent_cancel;
            let finished = Rc::clone(&self.finished);
            let action = Function::new(
                ctx.clone(),
                move |ctx: Ctx<'js>,
                      function: rquickjs::function::FuncArg<Function<'js>>|
                      -> Result<Value<'js>> {
                    if finished.replace(true) {
                        return Ok(Value::new_undefined(ctx));
                    }
                    let Some(inner) = weak.upgrade() else {
                        return Ok(Value::new_undefined(ctx));
                    };
                    if !released_early && !ReadableStream::reader_is_current(&inner, reader_id) {
                        return Ok(Value::new_undefined(ctx));
                    }
                    let reason: Value = function.get("reason")?;
                    let cancel = (!prevent_cancel)
                        .then(|| ReadableStream::cancel_stream(&ctx, &inner, reason))
                        .transpose()?;
                    ReadableStream::release_reader(&ctx, &inner, reader_id);
                    Ok(cancel.map_or_else(|| Value::new_undefined(ctx), Promise::into_value))
                },
            )?;
            action.set("reason", value)?;
            let sequence: Function = ctx.eval(
                "(previous, action, result) => previous.then(action, action).then(() => result)",
            )?;
            let promise: Promise = sequence.call((previous, action, result))?;
            *self.returning.borrow_mut() = Some(promise.clone());
            return Ok(promise);
        }
        self.finished.set(true);
        let cancel = (!self.prevent_cancel)
            .then(|| ReadableStream::cancel_stream(&ctx, &inner, value))
            .transpose()?;
        ReadableStream::release_reader(&ctx, &inner, self.reader_id);
        let promise = match cancel {
            Some(cancel) => after(&ctx, cancel, result)?,
            None => Cap::resolved(&ctx, result)?,
        };
        *self.returning.borrow_mut() = Some(promise.clone());
        Ok(promise)
    }

    #[qjs(rename = PredefinedAtom::SymbolAsyncIterator)]
    pub fn async_iterator(this: This<Value<'js>>) -> Value<'js> { this.0 }

    #[qjs(prop, rename = PredefinedAtom::SymbolToStringTag, configurable)]
    pub const fn to_string_tag() -> &'static str { "ReadableStreamAsyncIterator" }
}

// ---- ReadableStream.from -------------------------------------------------

pub fn from_iterable<'js>(
    ctx: &Ctx<'js>, iterable: Value<'js>,
) -> Result<Class<'js, ReadableStream<'js>>> {
    let object = iterable
        .as_object()
        .cloned()
        .ok_or_else(|| Exception::throw_type(ctx, "ReadableStream.from requires an iterable"))?;
    let async_key = rquickjs::Symbol::async_iterator(ctx.clone());
    let sync_key = rquickjs::Symbol::iterator(ctx.clone());
    let mut factory: Value = object.get(async_key)?;
    if factory.is_undefined() || factory.is_null() {
        factory = object.get(sync_key)?;
    }
    let factory = factory
        .into_function()
        .ok_or_else(|| Exception::throw_type(ctx, "ReadableStream.from requires an iterable"))?;
    let iterator: Object = factory.call((This(object),))?;
    let next: Function = iterator.get("next")?;
    // JS closures are traced by QuickJS and async functions already provide
    // exactly the two promise-flattening steps AsyncFromSyncIterator needs.
    let pull_factory: Function = ctx.eval(
        r#"(iterator, next) => async controller => {
            const step = await Reflect.apply(next, iterator, []);
            if ((typeof step !== "object" && typeof step !== "function") || step === null) {
                throw new TypeError("an iterator must yield an object");
            }
            if (step.done) controller.close();
            else controller.enqueue(await step.value);
        }"#,
    )?;
    let pull_fn: Function = pull_factory.call((iterator.clone(), next))?;
    let cancel_factory: Function = ctx.eval(
        r#"iterator => async reason => {
            const finish = iterator.return;
            if (finish == null) return;
            if (typeof finish !== "function") throw new TypeError("iterator.return must be callable");
            const result = await Reflect.apply(finish, iterator, [reason]);
            if ((typeof result !== "object" && typeof result !== "function") || result === null) {
                throw new TypeError("iterator.return must yield an object");
            }
        }"#,
    )?;
    let cancel_fn: Function = cancel_factory.call((iterator.clone(),))?;
    let inner = ReadableStream::new_inner(ctx)?;
    {
        let mut borrow = inner.borrow_mut();
        borrow.started = true;
        borrow.hwm = 0.0;
        borrow.source = iterator;
        borrow.pull_fn = Some(pull_fn);
        borrow.cancel_fn = Some(cancel_fn);
    }
    ReadableStream::attach_controller(ctx, &inner)?;
    ReadableStream::wrap(ctx, inner)
}
