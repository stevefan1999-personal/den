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
    Class, Ctx, Exception, Function, JsLifetime, Object, Promise, Result, Value,
    atom::PredefinedAtom,
    class::{Trace, Tracer},
    function::{Opt, This},
};

use crate::streams::{
    Cap, mark_handled, react,
    readable::{Inner as RsInner, ReadOutcome, ReadRequest, ReadableStream, iter_result},
    thrown, type_error,
    writable::{Inner as WsInner, WritableStream},
};

fn truthy(object: &Object<'_>, name: &str) -> Result<bool> {
    let value: Value = object.get(name)?;
    Ok(value.as_bool().unwrap_or_else(|| {
        !(value.is_undefined() || value.is_null() || value.is_bool())
            && value
                .as_number()
                .map(|number| number != 0.0)
                .unwrap_or(true)
            && value
                .as_string()
                .and_then(|s| s.to_string().ok())
                .is_none_or(|s| !s.is_empty())
    }))
}

/// The pipe holds both streams alive, as the specification's reader and writer
/// do. It is owned and traced by a single [`PipeRecord`] rooted on the source,
/// so the resulting cycle is one QuickJS can see and collect.
struct PipeState<'js> {
    source:         Option<Class<'js, ReadableStream<'js>>>,
    dest:           Option<Class<'js, WritableStream<'js>>>,
    reader_id:      u64,
    writer_id:      u64,
    prevent_close:  bool,
    prevent_abort:  bool,
    prevent_cancel: bool,
    cap:            Cap<'js>,
    shutting_down:  bool,
    signal:         Option<(Object<'js>, Function<'js>)>,
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
enum Shutdown {
    CloseDest,
    AbortDest,
    CancelSource,
    Both,
}

pub(crate) fn pipe_to<'js>(
    ctx: &Ctx<'js>, source: &Class<'js, ReadableStream<'js>>, dest: Option<Value<'js>>,
    options: Option<Value<'js>>,
) -> Result<Promise<'js>> {
    let dest = dest
        .as_ref()
        .and_then(Value::as_object)
        .and_then(Class::<WritableStream>::from_object)
        .ok_or_else(|| Exception::throw_type(ctx, "pipeTo requires a WritableStream"))?;
    let (prevent_close, prevent_abort, prevent_cancel, signal) = read_pipe_options(ctx, options)?;

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
        signal: None,
    }));

    if let Some(signal) = signal {
        let aborted: bool = signal.get("aborted").unwrap_or(false);
        if aborted {
            let reason: Value = signal
                .get("reason")
                .unwrap_or_else(|_| type_error(ctx, "the pipe was aborted"));
            abort_pipe(ctx, &state, reason);
            return Ok(promise);
        }
        let listener = {
            let state = Rc::downgrade(&state);
            let signal = signal.clone();
            Function::new(ctx.clone(), move |ctx: Ctx<'js>| {
                let Some(state) = state.upgrade() else {
                    return;
                };
                let reason: Value = signal
                    .get("reason")
                    .unwrap_or_else(|_| type_error(&ctx, "the pipe was aborted"));
                abort_pipe(&ctx, &state, reason);
            })?
        };
        if let Ok(add) = signal.get::<_, Function>("addEventListener") {
            let _ = add.call::<_, Value>((This(signal.clone()), "abort", listener.clone()));
        }
        state.borrow_mut().signal = Some((signal, listener));
    }

    let record = Class::instance(ctx.clone(), PipeRecord {
        state: Rc::clone(&state),
    })?;
    // Root the pipe on both ends. A running pipe must survive as long as
    // either stream can be observed, and rooting the destination too is what
    // keeps `pipeThrough` alive when script holds only the readable side.
    source_inner
        .borrow_mut()
        .roots
        .push(record.clone().into_value());
    dest_inner
        .borrow_mut()
        .roots
        .push(record.clone().into_value());
    let _ = record;
    step(ctx, &state);
    Ok(promise)
}

fn read_pipe_options<'js>(
    ctx: &Ctx<'js>, options: Option<Value<'js>>,
) -> Result<(bool, bool, bool, Option<Object<'js>>)> {
    let Some(object) = options
        .filter(|value| !value.is_undefined() && !value.is_null())
        .and_then(|value| value.into_object())
    else {
        return Ok((false, false, false, None));
    };
    let prevent_close = truthy(&object, "preventClose")?;
    let prevent_abort = truthy(&object, "preventAbort")?;
    let prevent_cancel = truthy(&object, "preventCancel")?;
    let signal: Value = object.get("signal")?;
    let signal = if signal.is_undefined() || signal.is_null() {
        None
    } else {
        let signal = signal
            .into_object()
            .filter(|object| object.contains_key("aborted").unwrap_or(false))
            .ok_or_else(|| Exception::throw_type(ctx, "signal must be an AbortSignal"))?;
        Some(signal)
    };
    Ok((prevent_close, prevent_abort, prevent_cancel, signal))
}

fn step<'js>(ctx: &Ctx<'js>, state: &Pipe<'js>) {
    if state.borrow().shutting_down {
        return;
    }
    let Some(dest_inner) = state.borrow().dest_inner() else {
        return;
    };
    // A destination that already failed or finished ends the pipe before the
    // next read, so a chunk is never read for a sink that cannot take it.
    if let Some(reason) = WritableStream::stored_error_for_pipe(&dest_inner) {
        shutdown(ctx, state, Some(reason), Shutdown::CancelSource);
        return;
    }
    if WritableStream::is_closed_for_pipe(&dest_inner) {
        let reason = type_error(ctx, "the destination stream is closed");
        shutdown(ctx, state, Some(reason), Shutdown::CancelSource);
        return;
    }
    let ready = WritableStream::writer_ready(&dest_inner);
    let on_ok = {
        let state = Rc::downgrade(state);
        Function::new(ctx.clone(), move |ctx: Ctx<'js>| {
            if let Some(state) = state.upgrade() {
                pump(&ctx, &state);
            }
        })
    };
    let on_err = {
        let state = Rc::downgrade(state);
        Function::new(ctx.clone(), move |ctx: Ctx<'js>, reason: Value<'js>| {
            if let Some(state) = state.upgrade() {
                shutdown(&ctx, &state, Some(reason), Shutdown::CancelSource);
            }
        })
    };
    match (ready, on_ok, on_err) {
        (Some(ready), Ok(on_ok), Ok(on_err)) => {
            let _ = react(ctx, ready.into_value(), Some(on_ok), Some(on_err));
        }
        _ => pump(ctx, state),
    }
}

fn pump<'js>(ctx: &Ctx<'js>, state: &Pipe<'js>) {
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
                        let Some(dest_inner) = state.borrow().dest_inner() else {
                            return;
                        };
                        match WritableStream::writer_write(ctx, &dest_inner, chunk) {
                            Ok(promise) => mark_handled(ctx, &promise),
                            Err(error) => {
                                let reason = thrown(ctx, error);
                                shutdown(ctx, &state, Some(reason), Shutdown::CancelSource);
                                return;
                            }
                        }
                        step(ctx, &state);
                    }
                    ReadOutcome::Close => shutdown(ctx, &state, None, Shutdown::CloseDest),
                    ReadOutcome::Error(reason) => {
                        shutdown(ctx, &state, Some(reason), Shutdown::AbortDest)
                    }
                }
            },
        ))
    };
    ReadableStream::read(ctx, &source_inner, request);
}

fn abort_pipe<'js>(ctx: &Ctx<'js>, state: &Pipe<'js>, reason: Value<'js>) {
    shutdown(ctx, state, Some(reason), Shutdown::Both);
}

fn shutdown<'js>(ctx: &Ctx<'js>, state: &Pipe<'js>, error: Option<Value<'js>>, action: Shutdown) {
    if state.borrow().shutting_down {
        return;
    }
    state.borrow_mut().shutting_down = true;
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
        finalize(ctx, state, error);
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
            if !prevent_abort {
                if let Ok(promise) = WritableStream::abort_stream(ctx, &dest_inner, reason.clone())
                {
                    mark_handled(ctx, &promise);
                }
            }
            if !prevent_cancel
                && let Ok(promise) = ReadableStream::cancel_stream(ctx, &source_inner, reason)
            {
                mark_handled(ctx, &promise);
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
                    finalize(&ctx, &state, failure);
                }
            };
            let on_ok = Function::new(ctx.clone(), finish(true));
            let on_err = Function::new(ctx.clone(), finish(false));
            match (on_ok, on_err) {
                (Ok(on_ok), Ok(on_err)) => {
                    let _ = react(ctx, promise.into_value(), Some(on_ok), Some(on_err));
                }
                _ => finalize(ctx, state, error),
            }
        }
        None => finalize(ctx, state, error),
    }
}

/// Drop this pipe's root from a stream that outlives it. A finished pipe left
/// in `roots` keeps its result capability — a JS promise no tracer reaches,
/// because the reactions holding it are `Function::new` closures — alive for
/// as long as the stream is, which QuickJS reports as a leak at teardown.
fn unroot<'js>(roots: &mut Vec<Value<'js>>, state: &Pipe<'js>) {
    roots.retain(|value| {
        Class::<PipeRecord<'js>>::from_value(value)
            .ok()
            .is_none_or(|record| !Rc::ptr_eq(&record.borrow().state, state))
    });
}

fn finalize<'js>(ctx: &Ctx<'js>, state: &Pipe<'js>, error: Option<Value<'js>>) {
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
        unroot(&mut inner.borrow_mut().roots, state);
    }
    if let Some(inner) = &dest_inner {
        unroot(&mut inner.borrow_mut().roots, state);
    }
    if let Some((signal, listener)) = signal
        && let Ok(remove) = signal.get::<_, Function>("removeEventListener")
    {
        let _ = remove.call::<_, Value>((This(signal.clone()), "abort", listener));
    }
    if let Some(dest_inner) = dest_inner {
        WritableStream::release_writer(ctx, &dest_inner, writer_id);
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

pub(crate) fn pipe_through<'js>(
    ctx: &Ctx<'js>, source: &Class<'js, ReadableStream<'js>>, transform: Option<Value<'js>>,
    options: Option<Value<'js>>,
) -> Result<Value<'js>> {
    let pair = transform
        .filter(|value| value.is_object())
        .and_then(Value::into_object)
        .ok_or_else(|| {
            Exception::throw_type(ctx, "pipeThrough requires a { readable, writable } pair")
        })?;
    let readable: Value = pair.get("readable")?;
    let writable: Value = pair.get("writable")?;
    if readable.as_object().is_none() || writable.as_object().is_none() {
        return Err(Exception::throw_type(
            ctx,
            "pipeThrough requires a { readable, writable } pair",
        ));
    }
    let promise = pipe_to(ctx, source, Some(writable), options)?;
    mark_handled(ctx, &promise);
    Ok(readable)
}

// ---- tee -----------------------------------------------------------------

/// Owned and traced by a single [`TeeRecord`] rooted on both branches.
struct TeeState<'js> {
    source:     Class<'js, ReadableStream<'js>>,
    reader_id:  u64,
    reading:    bool,
    read_again: bool,
    canceled:   [bool; 2],
    reasons:    [Option<Value<'js>>; 2],
    branches:   [Weak<RefCell<crate::streams::readable::ReadableInner<'js>>>; 2],
    cancel:     Option<Cap<'js>>,
}

impl<'js> Trace<'js> for TeeState<'js> {
    fn trace<'a>(&self, tracer: Tracer<'a, 'js>) {
        self.cancel.as_ref().map(|cap| cap.trace(tracer));
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

pub(crate) fn tee<'js>(
    ctx: &Ctx<'js>, source: &Class<'js, ReadableStream<'js>>,
) -> Result<(
    Class<'js, ReadableStream<'js>>,
    Class<'js, ReadableStream<'js>>,
)> {
    let source_inner = source.borrow().inner.clone();
    let reader_id = ReadableStream::acquire_reader(ctx, &source_inner)?;
    let state: Tee<'js> = Rc::new(RefCell::new(TeeState {
        source: source.clone(),
        reader_id,
        reading: false,
        read_again: false,
        canceled: [false, false],
        reasons: [None, None],
        branches: [Weak::new(), Weak::new()],
        cancel: None,
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
    state.borrow_mut().branches[index] = Rc::downgrade(&inner);
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
                    [borrow.branches[0].upgrade(), borrow.branches[1].upgrade()]
                };
                let canceled = state.borrow().canceled;
                match outcome {
                    ReadOutcome::Chunk(chunk) => {
                        for (index, branch) in branches.iter().enumerate() {
                            if canceled[index] {
                                continue;
                            }
                            if let Some(branch) = branch {
                                let _ = ReadableStream::enqueue(ctx, branch, chunk.clone());
                            }
                        }
                        let again = std::mem::take(&mut state.borrow_mut().read_again);
                        if again {
                            tee_pull(ctx, &state);
                        }
                    }
                    ReadOutcome::Close => {
                        for (index, branch) in branches.iter().enumerate() {
                            if canceled[index] {
                                continue;
                            }
                            if let Some(branch) = branch {
                                let _ = ReadableStream::close_requested(ctx, branch);
                            }
                        }
                        if (canceled[0] || canceled[1])
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
        borrow.canceled[index] = true;
        borrow.reasons[index] = Some(reason);
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
        let source_inner = state.borrow().source.borrow().inner.clone();
        let reader_id = state.borrow().reader_id;
        ReadableStream::release_reader(ctx, &source_inner, reader_id);
        let cancelled = ReadableStream::cancel_stream(ctx, &source_inner, composite.into_value())?;
        let outer = crate::streams::chain_undefined(ctx, cancelled.into_value())?;
        if let Some(cap) = state.borrow_mut().cancel.as_mut() {
            cap.fulfill(ctx);
        }
        return Ok(outer);
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
    finished:       Cell<bool>,
}

impl<'js> Trace<'js> for ReadableStreamAsyncIterator<'js> {
    fn trace<'a>(&self, tracer: Tracer<'a, 'js>) { self.stream.trace(tracer); }
}

pub(crate) fn values<'js>(
    ctx: &Ctx<'js>, stream: Class<'js, ReadableStream<'js>>, options: Option<Value<'js>>,
) -> Result<Class<'js, ReadableStreamAsyncIterator<'js>>> {
    let prevent_cancel = match options
        .filter(|value| !value.is_undefined() && !value.is_null())
        .and_then(Value::into_object)
    {
        Some(object) => truthy(&object, "preventCancel")?,
        None => false,
    };
    let inner = stream.borrow().inner.clone();
    let reader_id = ReadableStream::acquire_reader(ctx, &inner)?;
    Class::instance(ctx.clone(), ReadableStreamAsyncIterator {
        stream,
        reader_id,
        prevent_cancel,
        finished: Cell::new(false),
    })
}

#[rquickjs::methods(rename_all = "camelCase")]
impl<'js> ReadableStreamAsyncIterator<'js> {
    pub fn next(&self, ctx: Ctx<'js>) -> Result<Promise<'js>> {
        let inner = self.stream.borrow().inner.clone();
        if !ReadableStream::reader_is_current(&inner, self.reader_id) {
            return Cap::rejected(&ctx, type_error(&ctx, "the iterator was released"));
        }
        let cap = Cap::new(&ctx)?;
        let promise = cap.promise();
        ReadableStream::read(&ctx, &inner, ReadRequest::Js(cap));
        Ok(promise)
    }

    #[qjs(rename = "return")]
    pub fn iterator_return(&self, ctx: Ctx<'js>, value: Opt<Value<'js>>) -> Result<Promise<'js>> {
        let value = value.0.unwrap_or_else(|| Value::new_undefined(ctx.clone()));
        let inner = self.stream.borrow().inner.clone();
        if self.finished.replace(true) || !ReadableStream::reader_is_current(&inner, self.reader_id)
        {
            return Cap::resolved(&ctx, iter_result(&ctx, value, true)?);
        }
        if !self.prevent_cancel {
            let promise = ReadableStream::cancel_stream(&ctx, &inner, value.clone())?;
            mark_handled(&ctx, &promise);
        }
        ReadableStream::release_reader(&ctx, &inner, self.reader_id);
        Cap::resolved(&ctx, iter_result(&ctx, value, true)?)
    }

    #[qjs(rename = PredefinedAtom::SymbolAsyncIterator)]
    pub fn async_iterator(this: This<Value<'js>>) -> Value<'js> { this.0 }

    #[qjs(prop, rename = PredefinedAtom::SymbolToStringTag, configurable)]
    pub fn to_string_tag() -> &'static str { "ReadableStreamAsyncIterator" }
}

// ---- ReadableStream.from -------------------------------------------------

pub(crate) fn from_iterable<'js>(
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
    // Read once here for the specification's GetIterator validation; the pull
    // closure reads it again because it may capture no JS value of its own.
    let _: Function = iterator.get("next")?;
    let inner = ReadableStream::new_inner(ctx)?;
    {
        let mut borrow = inner.borrow_mut();
        borrow.started = true;
        borrow.source = iterator.clone();
        borrow.pull_fn = Some(Function::new(ctx.clone(), {
            let inner = Rc::downgrade(&inner);
            // The closure captures no JS value at all: `RustFunction` traces
            // nothing, so the iterator is read back out of the stream's own
            // `source` slot, which is traced, on every pull.
            move |ctx: Ctx<'js>| -> Result<()> {
                let Some(strong) = inner.upgrade() else {
                    return Ok(());
                };
                let iterator = strong.borrow().source.clone();
                let next: Function = iterator.get("next")?;
                let step: Value = next.call((This(iterator),))?;
                drop(strong);
                // Weak again, not the handle just upgraded: this closure is a
                // `RustFunction` with an empty `Trace` impl, so a strong record
                // here would hide the stream from the collector until the
                // iterator settles — which a stalled generator never does.
                let inner = inner.clone();
                let on_err = {
                    let inner = inner.clone();
                    Function::new(ctx.clone(), move |ctx: Ctx<'js>, reason: Value<'js>| {
                        if let Some(inner) = inner.upgrade() {
                            ReadableStream::error(&ctx, &inner, reason);
                        }
                    })?
                };
                let on_ok = Function::new(ctx.clone(), move |ctx: Ctx<'js>, step: Value<'js>| {
                    let Some(inner) = inner.upgrade() else {
                        return;
                    };
                    let Some(object) = step.as_object() else {
                        let reason = type_error(&ctx, "an iterator must yield an object");
                        ReadableStream::error(&ctx, &inner, reason);
                        return;
                    };
                    if object.get::<_, bool>("done").unwrap_or(false) {
                        let _ = ReadableStream::close_requested(&ctx, &inner);
                        return;
                    }
                    let value: Value = object
                        .get("value")
                        .unwrap_or_else(|_| Value::new_undefined(ctx.clone()));
                    let _ = ReadableStream::enqueue(&ctx, &inner, value);
                })?;
                react(&ctx, step, Some(on_ok), Some(on_err))?;
                Ok(())
            }
        })?);
        borrow.cancel_fn = Some(Function::new(ctx.clone(), {
            let inner = Rc::downgrade(&inner);
            move |ctx: Ctx<'js>, reason: Opt<Value<'js>>| -> Result<Value<'js>> {
                let Some(strong) = inner.upgrade() else {
                    return Ok(Value::new_undefined(ctx.clone()));
                };
                let iterator = strong.borrow().source.clone();
                drop(strong);
                match iterator.get::<_, Function>("return") {
                    Ok(finish) => finish.call((This(iterator), reason.0)),
                    Err(_) => Ok(Value::new_undefined(ctx.clone())),
                }
            }
        })?);
    }
    ReadableStream::attach_controller(ctx, &inner)?;
    ReadableStream::wrap(ctx, inner)
}
