//! `ReadableStream`, its default reader and its default controller.

use std::{
    cell::RefCell,
    rc::{Rc, Weak},
};

use rquickjs::{
    Class, Coerced, Ctx, Exception, FromJs, Function, JsLifetime, Object, Promise, Result, Value,
    atom::PredefinedAtom,
    class::{Trace, Tracer},
    function::{Opt, This},
};

use crate::streams::{Cap, method, native::NativeSource, pipe, react, thrown, type_error};

pub(crate) enum RsState<'js> {
    Readable,
    Closed,
    Errored(Value<'js>),
}

/// What a read request is handed. The native variant is the specification's
/// read-request record: pipe, tee, `read_all_bytes` and the async iterator all
/// use it, so those paths never mint a `{ value, done }` object.
pub(crate) enum ReadOutcome<'js> {
    Chunk(Value<'js>),
    Close,
    Error(Value<'js>),
}

pub(crate) enum ReadRequest<'js> {
    Js(Cap<'js>),
    Native(Box<dyn FnOnce(&Ctx<'js>, ReadOutcome<'js>) + 'js>),
}

impl<'js> Trace<'js> for ReadRequest<'js> {
    fn trace<'a>(&self, tracer: Tracer<'a, 'js>) {
        if let Self::Js(cap) = self {
            cap.trace(tracer);
        }
    }
}

impl<'js> ReadRequest<'js> {
    fn deliver(self, ctx: &Ctx<'js>, outcome: ReadOutcome<'js>) {
        match self {
            Self::Js(mut cap) => {
                match outcome {
                    ReadOutcome::Chunk(value) => {
                        if let Ok(result) = iter_result(ctx, value, false) {
                            cap.resolve(result);
                        }
                    }
                    ReadOutcome::Close => {
                        if let Ok(result) =
                            iter_result(ctx, Value::new_undefined(ctx.clone()), true)
                        {
                            cap.resolve(result);
                        }
                    }
                    ReadOutcome::Error(reason) => cap.reject(reason),
                }
            }
            Self::Native(deliver) => deliver(ctx, outcome),
        }
    }
}

pub(crate) fn iter_result<'js>(
    ctx: &Ctx<'js>, value: Value<'js>, done: bool,
) -> Result<Value<'js>> {
    let object = Object::new(ctx.clone())?;
    object.set("value", value)?;
    object.set("done", done)?;
    Ok(object.into_value())
}

pub(crate) struct ReaderSlot<'js> {
    id:     u64,
    closed: Cap<'js>,
}

impl<'js> Trace<'js> for ReaderSlot<'js> {
    fn trace<'a>(&self, tracer: Tracer<'a, 'js>) { self.closed.trace(tracer); }
}

pub(crate) struct ReadableInner<'js> {
    pub(crate) state:           RsState<'js>,
    pub(crate) disturbed:       bool,
    pub(crate) reader:          Option<ReaderSlot<'js>>,
    pub(crate) next_reader:     u64,
    pub(crate) read_requests:   Vec<ReadRequest<'js>>,
    pub(crate) controller:      Option<Class<'js, ReadableStreamDefaultController<'js>>>,
    pub(crate) queue:           Vec<(Value<'js>, f64)>,
    pub(crate) queue_total:     f64,
    pub(crate) hwm:             f64,
    pub(crate) size_fn:         Option<Function<'js>>,
    pub(crate) source:          Object<'js>,
    pub(crate) pull_fn:         Option<Function<'js>>,
    pub(crate) cancel_fn:       Option<Function<'js>>,
    pub(crate) native:          Option<Rc<RefCell<NativeSource<'js>>>>,
    pub(crate) started:         bool,
    pub(crate) pulling:         bool,
    pub(crate) pull_again:      bool,
    pub(crate) close_requested: bool,
    /// Objects the stream must keep alive and, crucially, trace: any Rust
    /// record that holds JS values (a transform's shared state, a pipe, a tee)
    /// is owned by exactly one JS class instance which is rooted here.
    pub(crate) roots:           Vec<Value<'js>>,
}

impl<'js> Trace<'js> for ReadableInner<'js> {
    fn trace<'a>(&self, tracer: Tracer<'a, 'js>) {
        if let RsState::Errored(reason) = &self.state {
            reason.trace(tracer);
        }
        self.reader.as_ref().map(|slot| slot.trace(tracer));
        for request in &self.read_requests {
            request.trace(tracer);
        }
        self.controller.trace(tracer);
        for (chunk, _) in &self.queue {
            chunk.trace(tracer);
        }
        self.size_fn.trace(tracer);
        self.source.trace(tracer);
        self.pull_fn.trace(tracer);
        self.cancel_fn.trace(tracer);
        self.roots.trace(tracer);
    }
}

// SAFETY: every field is either plain data or a `'js` JS handle, so the type
// is covariant in `'js` exactly as the derive would generate.
unsafe impl<'js> rquickjs::JsLifetime<'js> for ReadableInner<'js> {
    type Changed<'to> = ReadableInner<'to>;
}

pub(crate) type Inner<'js> = Rc<RefCell<ReadableInner<'js>>>;

#[derive(JsLifetime)]
#[rquickjs::class]
pub struct ReadableStream<'js> {
    pub(crate) inner: Inner<'js>,
}

impl<'js> Trace<'js> for ReadableStream<'js> {
    fn trace<'a>(&self, tracer: Tracer<'a, 'js>) {
        if let Ok(inner) = self.inner.try_borrow() {
            inner.trace(tracer);
        }
    }
}

/// Parse a `{ highWaterMark, size }` strategy in specification order.
pub(crate) fn extract_strategy<'js>(
    ctx: &Ctx<'js>, strategy: Option<Value<'js>>, default_hwm: f64,
) -> Result<(f64, Option<Function<'js>>)> {
    let Some(object) = strategy
        .filter(Value::is_object)
        .and_then(Value::into_object)
    else {
        return Ok((default_hwm, None));
    };
    let size: Value = object.get("size")?;
    let size_fn = if size.is_undefined() {
        None
    } else {
        Some(
            size.into_function()
                .ok_or_else(|| Exception::throw_type(ctx, "strategy size must be a function"))?,
        )
    };
    let mark: Value = object.get("highWaterMark")?;
    let hwm = if mark.is_undefined() {
        default_hwm
    } else {
        let number = Coerced::<f64>::from_js(ctx, mark)?.0;
        if number.is_nan() || number < 0.0 {
            return Err(Exception::throw_range(
                ctx,
                "highWaterMark must be a non-negative number",
            ));
        }
        number
    };
    Ok((hwm, size_fn))
}

impl<'js> ReadableStream<'js> {
    pub(crate) fn new_inner(ctx: &Ctx<'js>) -> Result<Inner<'js>> {
        Ok(Rc::new(RefCell::new(ReadableInner {
            state:           RsState::Readable,
            disturbed:       false,
            reader:          None,
            next_reader:     1,
            read_requests:   Vec::new(),
            controller:      None,
            queue:           Vec::new(),
            queue_total:     0.0,
            hwm:             1.0,
            size_fn:         None,
            source:          Object::new(ctx.clone())?,
            pull_fn:         None,
            cancel_fn:       None,
            native:          None,
            started:         false,
            pulling:         false,
            pull_again:      false,
            close_requested: false,
            roots:           Vec::new(),
        })))
    }

    pub(crate) fn is_locked(inner: &Inner<'js>) -> bool { inner.borrow().reader.is_some() }

    /// Give a stream its default controller. Every path that builds a stream
    /// from Rust needs one, because `pull` is handed the controller.
    pub(crate) fn attach_controller(ctx: &Ctx<'js>, inner: &Inner<'js>) -> Result<()> {
        let controller = Class::instance(ctx.clone(), ReadableStreamDefaultController {
            inner: Rc::downgrade(inner),
        })?;
        inner.borrow_mut().controller = Some(controller);
        Ok(())
    }

    pub(crate) fn stored_error(inner: &Inner<'js>) -> Option<Value<'js>> {
        match &inner.borrow().state {
            RsState::Errored(reason) => Some(reason.clone()),
            _ => None,
        }
    }

    /// Take a reader slot without handing a reader object to script. Used by
    /// pipe, tee, the async iterator and `den:fetch`'s body consumption.
    pub(crate) fn acquire_reader(ctx: &Ctx<'js>, inner: &Inner<'js>) -> Result<u64> {
        if Self::is_locked(inner) {
            return Err(Exception::throw_type(ctx, "ReadableStream is locked"));
        }
        let mut closed = Cap::new(ctx)?;
        let (id, state) = {
            let mut borrow = inner.borrow_mut();
            let id = borrow.next_reader;
            borrow.next_reader += 1;
            let state = match &borrow.state {
                RsState::Closed => Some(None),
                RsState::Errored(reason) => Some(Some(reason.clone())),
                RsState::Readable => None,
            };
            (id, state)
        };
        match state {
            Some(None) => closed.fulfill(ctx),
            Some(Some(reason)) => closed.reject_handled(ctx, reason),
            None => {}
        }
        inner.borrow_mut().reader = Some(ReaderSlot { id, closed });
        Ok(id)
    }

    pub(crate) fn reader_is_current(inner: &Inner<'js>, id: u64) -> bool {
        inner
            .borrow()
            .reader
            .as_ref()
            .is_some_and(|slot| slot.id == id)
    }

    pub(crate) fn release_reader(ctx: &Ctx<'js>, inner: &Inner<'js>, id: u64) {
        if !Self::reader_is_current(inner, id) {
            return;
        }
        let slot = inner.borrow_mut().reader.take();
        let reason = type_error(ctx, "the reader was released");
        if let Some(mut slot) = slot
            && slot.closed.is_pending()
        {
            slot.closed.reject_handled(ctx, reason.clone());
        }
        let requests = std::mem::take(&mut inner.borrow_mut().read_requests);
        for request in requests {
            request.deliver(ctx, ReadOutcome::Error(reason.clone()));
        }
    }

    pub(crate) fn closed_promise(ctx: &Ctx<'js>, inner: &Inner<'js>) -> Result<Promise<'js>> {
        match inner.borrow().reader.as_ref() {
            Some(slot) => Ok(slot.closed.promise()),
            None => Cap::rejected(ctx, type_error(ctx, "the reader was released")),
        }
    }

    // ---- controller algorithms ------------------------------------------

    pub(crate) fn desired_size(inner: &Inner<'js>) -> Option<f64> {
        let borrow = inner.borrow();
        match borrow.state {
            RsState::Errored(_) => None,
            RsState::Closed => Some(0.0),
            RsState::Readable => Some(borrow.hwm - borrow.queue_total),
        }
    }

    fn clear_algorithms(inner: &Inner<'js>) {
        let mut borrow = inner.borrow_mut();
        borrow.pull_fn = None;
        borrow.cancel_fn = None;
        borrow.size_fn = None;
        borrow.native = None;
    }

    fn should_pull(inner: &Inner<'js>) -> bool {
        let borrow = inner.borrow();
        if !borrow.started || borrow.close_requested {
            return false;
        }
        if !matches!(borrow.state, RsState::Readable) {
            return false;
        }
        if !borrow.read_requests.is_empty() {
            return true;
        }
        borrow.hwm - borrow.queue_total > 0.0
    }

    pub(crate) fn pull_if_needed(ctx: &Ctx<'js>, inner: &Inner<'js>) {
        if !Self::should_pull(inner) {
            return;
        }
        {
            let mut borrow = inner.borrow_mut();
            if borrow.pulling {
                borrow.pull_again = true;
                return;
            }
            borrow.pulling = true;
        }
        if inner.borrow().native.is_some() {
            crate::streams::native::drive_pull(ctx, inner);
            return;
        }
        let (pull_fn, source, controller) = {
            let borrow = inner.borrow();
            (
                borrow.pull_fn.clone(),
                borrow.source.clone(),
                borrow.controller.clone(),
            )
        };
        let outcome = match (pull_fn, controller) {
            (Some(pull), Some(controller)) => pull.call::<_, Value>((This(source), controller)),
            _ => Ok(Value::new_undefined(ctx.clone())),
        };
        match outcome {
            Ok(value) => {
                let ok = {
                    // A pending reaction keeps the stream's record alive: the operation it is
                    // waiting on is part of the stream, and a weak handle here would silently
                    // abandon a write or a pull whose stream became unreachable mid-flight.
                    let inner = Rc::clone(inner);
                    Function::new(ctx.clone(), move |ctx: Ctx<'js>| {
                        ReadableStream::pull_settled(&ctx, &inner);
                    })
                };
                let err = {
                    // A pending reaction keeps the stream's record alive: the operation it is
                    // waiting on is part of the stream, and a weak handle here would silently
                    // abandon a write or a pull whose stream became unreachable mid-flight.
                    let inner = Rc::clone(inner);
                    Function::new(ctx.clone(), move |ctx: Ctx<'js>, reason: Value<'js>| {
                        inner.borrow_mut().pulling = false;
                        ReadableStream::error(&ctx, &inner, reason);
                    })
                };
                if let (Ok(ok), Ok(err)) = (ok, err) {
                    let _ = react(ctx, value, Some(ok), Some(err));
                }
            }
            Err(error) => {
                inner.borrow_mut().pulling = false;
                let reason = thrown(ctx, error);
                Self::error(ctx, inner, reason);
            }
        }
    }

    pub(crate) fn pull_settled(ctx: &Ctx<'js>, inner: &Inner<'js>) {
        let again = {
            let mut borrow = inner.borrow_mut();
            borrow.pulling = false;
            std::mem::take(&mut borrow.pull_again)
        };
        if again {
            Self::pull_if_needed(ctx, inner);
        }
    }

    pub(crate) fn enqueue(ctx: &Ctx<'js>, inner: &Inner<'js>, chunk: Value<'js>) -> Result<()> {
        {
            let borrow = inner.borrow();
            if borrow.close_requested || !matches!(borrow.state, RsState::Readable) {
                return Err(Exception::throw_type(
                    ctx,
                    "the stream is closing or not readable",
                ));
            }
        }
        let waiting = !inner.borrow().read_requests.is_empty();
        if waiting {
            Self::fulfill_read(ctx, inner, chunk);
        } else {
            let size_fn = inner.borrow().size_fn.clone();
            let size = match size_fn {
                Some(size_fn) => {
                    match size_fn.call::<_, Coerced<f64>>((chunk.clone(),)) {
                        Ok(size) => {
                            if !size.0.is_finite() || size.0 < 0.0 {
                                let reason = type_error(
                                    ctx,
                                    "a chunk size must be a non-negative finite number",
                                );
                                Self::error(ctx, inner, reason.clone());
                                return Err(ctx.throw(reason));
                            }
                            size.0
                        }
                        Err(error) => {
                            let reason = thrown(ctx, error);
                            Self::error(ctx, inner, reason.clone());
                            return Err(ctx.throw(reason));
                        }
                    }
                }
                None => 1.0,
            };
            let mut borrow = inner.borrow_mut();
            borrow.queue.push((chunk, size));
            borrow.queue_total += size;
        }
        Self::pull_if_needed(ctx, inner);
        Ok(())
    }

    fn fulfill_read(ctx: &Ctx<'js>, inner: &Inner<'js>, chunk: Value<'js>) {
        let request = {
            let mut borrow = inner.borrow_mut();
            if borrow.read_requests.is_empty() {
                None
            } else {
                Some(borrow.read_requests.remove(0))
            }
        };
        if let Some(request) = request {
            request.deliver(ctx, ReadOutcome::Chunk(chunk));
        }
    }

    pub(crate) fn close_requested(ctx: &Ctx<'js>, inner: &Inner<'js>) -> Result<()> {
        {
            let borrow = inner.borrow();
            if borrow.close_requested || !matches!(borrow.state, RsState::Readable) {
                return Err(Exception::throw_type(
                    ctx,
                    "the stream is already closing or not readable",
                ));
            }
        }
        inner.borrow_mut().close_requested = true;
        if inner.borrow().queue.is_empty() {
            Self::clear_algorithms(inner);
            Self::close(ctx, inner);
        }
        Ok(())
    }

    pub(crate) fn close(ctx: &Ctx<'js>, inner: &Inner<'js>) {
        if !matches!(inner.borrow().state, RsState::Readable) {
            return;
        }
        inner.borrow_mut().state = RsState::Closed;
        if let Some(slot) = inner.borrow_mut().reader.as_mut() {
            slot.closed.fulfill(ctx);
        }
        let requests = std::mem::take(&mut inner.borrow_mut().read_requests);
        for request in requests {
            request.deliver(ctx, ReadOutcome::Close);
        }
    }

    pub(crate) fn error(ctx: &Ctx<'js>, inner: &Inner<'js>, reason: Value<'js>) {
        if !matches!(inner.borrow().state, RsState::Readable) {
            return;
        }
        {
            let mut borrow = inner.borrow_mut();
            borrow.queue.clear();
            borrow.queue_total = 0.0;
            borrow.state = RsState::Errored(reason.clone());
        }
        Self::clear_algorithms(inner);
        if let Some(slot) = inner.borrow_mut().reader.as_mut() {
            slot.closed.reject_handled(ctx, reason.clone());
        }
        let requests = std::mem::take(&mut inner.borrow_mut().read_requests);
        for request in requests {
            request.deliver(ctx, ReadOutcome::Error(reason.clone()));
        }
    }

    pub(crate) fn read(ctx: &Ctx<'js>, inner: &Inner<'js>, request: ReadRequest<'js>) {
        inner.borrow_mut().disturbed = true;
        let outcome = {
            let borrow = inner.borrow();
            match &borrow.state {
                RsState::Closed => Some(ReadOutcome::Close),
                RsState::Errored(reason) => Some(ReadOutcome::Error(reason.clone())),
                RsState::Readable => None,
            }
        };
        if let Some(outcome) = outcome {
            request.deliver(ctx, outcome);
            return;
        }
        let chunk = {
            let mut borrow = inner.borrow_mut();
            if borrow.queue.is_empty() {
                None
            } else {
                let (chunk, size) = borrow.queue.remove(0);
                borrow.queue_total = (borrow.queue_total - size).max(0.0);
                Some(chunk)
            }
        };
        match chunk {
            Some(chunk) => {
                let drained = {
                    let borrow = inner.borrow();
                    borrow.close_requested && borrow.queue.is_empty()
                };
                if drained {
                    Self::clear_algorithms(inner);
                    Self::close(ctx, inner);
                } else {
                    Self::pull_if_needed(ctx, inner);
                }
                request.deliver(ctx, ReadOutcome::Chunk(chunk));
            }
            None => {
                inner.borrow_mut().read_requests.push(request);
                Self::pull_if_needed(ctx, inner);
            }
        }
    }

    pub(crate) fn cancel_stream(
        ctx: &Ctx<'js>, inner: &Inner<'js>, reason: Value<'js>,
    ) -> Result<Promise<'js>> {
        inner.borrow_mut().disturbed = true;
        match &inner.borrow().state {
            RsState::Closed => return Cap::undefined(ctx),
            RsState::Errored(stored) => return Cap::rejected(ctx, stored.clone()),
            RsState::Readable => {}
        }
        Self::close(ctx, inner);
        {
            let mut borrow = inner.borrow_mut();
            borrow.queue.clear();
            borrow.queue_total = 0.0;
        }
        let (cancel_fn, source, native) = {
            let borrow = inner.borrow();
            (
                borrow.cancel_fn.clone(),
                borrow.source.clone(),
                borrow.native.clone(),
            )
        };
        if let Some(native) = native {
            native.borrow_mut().cancel(reason.clone());
        }
        let outcome = match cancel_fn {
            Some(cancel) => cancel.call::<_, Value>((This(source), reason)),
            None => Ok(Value::new_undefined(ctx.clone())),
        };
        Self::clear_algorithms(inner);
        match outcome {
            Ok(value) => crate::streams::chain_undefined(ctx, value),
            Err(error) => Cap::rejected(ctx, thrown(ctx, error)),
        }
    }

    // ---- Rust API used by den:fetch and den:whatwg ------------------------

    pub fn lock_for_consume(stream: &Class<'js, Self>, ctx: &Ctx<'js>) -> Result<()> {
        let inner = stream.borrow().inner.clone();
        Self::acquire_reader(ctx, &inner).map(|_| ())
    }

    pub fn from_queue(ctx: &Ctx<'js>, queue: Vec<Value<'js>>) -> Result<Class<'js, Self>> {
        let inner = Self::new_inner(ctx)?;
        {
            let mut borrow = inner.borrow_mut();
            borrow.started = true;
            borrow.close_requested = true;
            borrow.queue_total = queue.len() as f64;
            borrow.queue = queue.into_iter().map(|value| (value, 1.0)).collect();
            if borrow.queue.is_empty() {
                borrow.state = RsState::Closed;
            }
        }
        Class::instance(ctx.clone(), Self { inner })
    }

    pub fn tee_pair(stream: &Class<'js, Self>, ctx: &Ctx<'js>) -> Result<(Value<'js>, Value<'js>)> {
        let (left, right) = pipe::tee(ctx, stream)?;
        Ok((left.into_value(), right.into_value()))
    }

    pub async fn read_all_bytes(stream: &Class<'js, Self>, ctx: Ctx<'js>) -> Result<Vec<u8>> {
        let inner = stream.borrow().inner.clone();
        let mut out = Vec::new();
        loop {
            let cap = Cap::new(&ctx)?;
            let promise = cap.promise();
            Self::read(&ctx, &inner, ReadRequest::Js(cap));
            let result: Value = promise.into_future().await?;
            let Some(object) = result.as_object() else {
                break;
            };
            if object.get::<_, bool>("done").unwrap_or(false) {
                break;
            }
            let value: Value = object.get("value")?;
            let Some(bytes) = crate::host::Host::buffer_source_bytes(&ctx, value)? else {
                return Err(Exception::throw_type(
                    &ctx,
                    "ReadableStream chunk must be a Uint8Array",
                ));
            };
            out.extend(bytes);
        }
        Ok(out)
    }
}

#[rquickjs::methods(rename_all = "camelCase")]
impl<'js> ReadableStream<'js> {
    #[qjs(constructor)]
    pub fn new(ctx: Ctx<'js>, source: Opt<Value<'js>>, strategy: Opt<Value<'js>>) -> Result<Self> {
        let source_object = match source.0.clone() {
            Some(value) if value.is_object() => {
                value.into_object().unwrap_or(Object::new(ctx.clone())?)
            }
            _ => Object::new(ctx.clone())?,
        };
        let kind: Value = source_object.get("type")?;
        if !kind.is_undefined() {
            let name = den_util::coerce_string(&ctx, kind)?;
            return Err(Exception::throw_type(
                &ctx,
                &format!(
                    "den: ReadableStream type `{name}` is not implemented (byte streams need \
                     ArrayBuffer transfer, which QuickJS does not expose)"
                ),
            ));
        }
        let (hwm, size_fn) = extract_strategy(&ctx, strategy.0, 1.0)?;
        let start_fn = method(&ctx, &source_object, "start")?;
        let pull_fn = method(&ctx, &source_object, "pull")?;
        let cancel_fn = method(&ctx, &source_object, "cancel")?;
        let inner = Self::new_inner(&ctx)?;
        {
            let mut borrow = inner.borrow_mut();
            borrow.hwm = hwm;
            borrow.size_fn = size_fn;
            borrow.source = source_object.clone();
            borrow.pull_fn = pull_fn;
            borrow.cancel_fn = cancel_fn;
        }
        let controller = Class::instance(ctx.clone(), ReadableStreamDefaultController {
            inner: Rc::downgrade(&inner),
        })?;
        inner.borrow_mut().controller = Some(controller.clone());
        let started = match start_fn {
            Some(start) => start.call::<_, Value>((This(source_object), controller))?,
            None => Value::new_undefined(ctx.clone()),
        };
        let ok = {
            let inner = Rc::clone(&inner);
            Function::new(ctx.clone(), move |ctx: Ctx<'js>| {
                inner.borrow_mut().started = true;
                ReadableStream::pull_settled(&ctx, &inner);
                ReadableStream::pull_if_needed(&ctx, &inner);
            })?
        };
        let err = {
            let inner = Rc::clone(&inner);
            Function::new(ctx.clone(), move |ctx: Ctx<'js>, reason: Value<'js>| {
                ReadableStream::error(&ctx, &inner, reason);
            })?
        };
        react(&ctx, started, Some(ok), Some(err))?;
        Ok(Self { inner })
    }

    #[qjs(static)]
    pub fn from(ctx: Ctx<'js>, iterable: Value<'js>) -> Result<Class<'js, Self>> {
        pipe::from_iterable(&ctx, iterable)
    }

    #[qjs(get)]
    pub fn locked(&self) -> bool { Self::is_locked(&self.inner) }

    pub fn is_disturbed(&self) -> bool { self.inner.borrow().disturbed }

    #[qjs(get, rename = "_denDisturbed")]
    pub fn den_disturbed(&self) -> bool { self.is_disturbed() }

    /// den extension: error the stream from the host (a fetch abort).
    #[qjs(rename = "_denAbort")]
    pub fn den_abort(&self, ctx: Ctx<'js>, reason: Opt<Value<'js>>) {
        let reason = reason
            .0
            .filter(|value| !value.is_undefined())
            .unwrap_or_else(|| type_error(&ctx, "the stream was aborted"));
        Self::error(&ctx, &self.inner, reason);
    }

    pub fn cancel(&self, ctx: Ctx<'js>, reason: Opt<Value<'js>>) -> Result<Promise<'js>> {
        if Self::is_locked(&self.inner) {
            return Cap::rejected(&ctx, type_error(&ctx, "ReadableStream is locked"));
        }
        Self::cancel_stream(
            &ctx,
            &self.inner,
            reason
                .0
                .unwrap_or_else(|| Value::new_undefined(ctx.clone())),
        )
    }

    pub fn get_reader(
        this: This<Class<'js, Self>>, ctx: Ctx<'js>, options: Opt<Value<'js>>,
    ) -> Result<Class<'js, ReadableStreamDefaultReader<'js>>> {
        if let Some(object) = options.0.as_ref().and_then(Value::as_object) {
            let mode: Value = object.get("mode")?;
            if !mode.is_undefined() {
                let mode = den_util::coerce_string(&ctx, mode)?;
                if mode == "byob" {
                    return Err(Exception::throw_type(
                        &ctx,
                        "den: BYOB readers are not implemented",
                    ));
                }
                return Err(Exception::throw_type(&ctx, "invalid reader mode"));
            }
        }
        ReadableStreamDefaultReader::acquire(&ctx, this.0)
    }

    pub fn tee(this: This<Class<'js, Self>>, ctx: Ctx<'js>) -> Result<rquickjs::Array<'js>> {
        let (left, right) = pipe::tee(&ctx, &this.0)?;
        let pair = rquickjs::Array::new(ctx)?;
        pair.set(0, left)?;
        pair.set(1, right)?;
        Ok(pair)
    }

    pub fn pipe_to(
        this: This<Class<'js, Self>>, ctx: Ctx<'js>, dest: Opt<Value<'js>>,
        options: Opt<Value<'js>>,
    ) -> Result<Promise<'js>> {
        match pipe::pipe_to(&ctx, &this.0, dest.0, options.0) {
            Ok(promise) => Ok(promise),
            Err(error) => Cap::rejected(&ctx, thrown(&ctx, error)),
        }
    }

    pub fn pipe_through(
        this: This<Class<'js, Self>>, ctx: Ctx<'js>, transform: Opt<Value<'js>>,
        options: Opt<Value<'js>>,
    ) -> Result<Value<'js>> {
        pipe::pipe_through(&ctx, &this.0, transform.0, options.0)
    }

    pub fn values(
        this: This<Class<'js, Self>>, ctx: Ctx<'js>, options: Opt<Value<'js>>,
    ) -> Result<Class<'js, ReadableStreamAsyncIterator<'js>>> {
        pipe::values(&ctx, this.0, options.0)
    }

    #[qjs(rename = PredefinedAtom::SymbolAsyncIterator)]
    pub fn async_iterator(
        this: This<Class<'js, Self>>, ctx: Ctx<'js>, options: Opt<Value<'js>>,
    ) -> Result<Class<'js, ReadableStreamAsyncIterator<'js>>> {
        pipe::values(&ctx, this.0, options.0)
    }

    #[qjs(prop, rename = PredefinedAtom::SymbolToStringTag, configurable)]
    pub fn to_string_tag() -> &'static str { "ReadableStream" }
}

#[rquickjs::class]
pub struct ReadableStreamDefaultController<'js> {
    /// Weak on purpose. The stream owns the controller object and traces it;
    /// a strong `Rc` back would be a reference cycle running through a JS
    /// object, which neither QuickJS's collector nor `Rc` can break.
    inner: Weak<RefCell<ReadableInner<'js>>>,
}

/// The controller does not trace its stream: the stream is the single JS owner
/// of every value in `ReadableInner`, and tracing the same reference twice
/// would double-decrement it during a mark phase.
// SAFETY: the weak handle is `'js`-scoped, exactly like the strong one.
unsafe impl<'js> rquickjs::JsLifetime<'js> for ReadableStreamDefaultController<'js> {
    type Changed<'to> = ReadableStreamDefaultController<'to>;
}

impl<'js> Trace<'js> for ReadableStreamDefaultController<'js> {
    fn trace<'a>(&self, _tracer: Tracer<'a, 'js>) {}
}

impl<'js> ReadableStreamDefaultController<'js> {
    fn stream(&self, ctx: &Ctx<'js>) -> Result<Inner<'js>> {
        self.inner
            .upgrade()
            .ok_or_else(|| Exception::throw_type(ctx, "the stream is gone"))
    }
}

#[rquickjs::methods(rename_all = "camelCase")]
impl<'js> ReadableStreamDefaultController<'js> {
    #[qjs(constructor)]
    pub fn new(ctx: Ctx<'js>) -> Result<Self> {
        Err(Exception::throw_type(&ctx, "Illegal constructor"))
    }

    #[qjs(get)]
    pub fn desired_size(&self, ctx: Ctx<'js>) -> Value<'js> {
        match self
            .inner
            .upgrade()
            .and_then(|inner| ReadableStream::desired_size(&inner))
        {
            Some(size) => Value::new_float(ctx, size),
            None => Value::new_null(ctx),
        }
    }

    pub fn enqueue(&self, ctx: Ctx<'js>, chunk: Opt<Value<'js>>) -> Result<()> {
        let inner = self.stream(&ctx)?;
        ReadableStream::enqueue(
            &ctx,
            &inner,
            chunk.0.unwrap_or_else(|| Value::new_undefined(ctx.clone())),
        )
    }

    pub fn close(&self, ctx: Ctx<'js>) -> Result<()> {
        let inner = self.stream(&ctx)?;
        ReadableStream::close_requested(&ctx, &inner)
    }

    pub fn error(&self, ctx: Ctx<'js>, reason: Opt<Value<'js>>) {
        if let Some(inner) = self.inner.upgrade() {
            ReadableStream::error(
                &ctx,
                &inner,
                reason
                    .0
                    .unwrap_or_else(|| Value::new_undefined(ctx.clone())),
            );
        }
    }

    #[qjs(prop, rename = PredefinedAtom::SymbolToStringTag, configurable)]
    pub fn to_string_tag() -> &'static str { "ReadableStreamDefaultController" }
}

#[derive(JsLifetime)]
#[rquickjs::class]
pub struct ReadableStreamDefaultReader<'js> {
    stream: Class<'js, ReadableStream<'js>>,
    id:     u64,
    closed: Promise<'js>,
}

impl<'js> Trace<'js> for ReadableStreamDefaultReader<'js> {
    fn trace<'a>(&self, tracer: Tracer<'a, 'js>) {
        self.stream.trace(tracer);
        self.closed.trace(tracer);
    }
}

impl<'js> ReadableStreamDefaultReader<'js> {
    pub(crate) fn acquire(
        ctx: &Ctx<'js>, stream: Class<'js, ReadableStream<'js>>,
    ) -> Result<Class<'js, Self>> {
        let inner = stream.borrow().inner.clone();
        let id = ReadableStream::acquire_reader(ctx, &inner)?;
        let closed = ReadableStream::closed_promise(ctx, &inner)?;
        Class::instance(ctx.clone(), Self { stream, id, closed })
    }

    fn inner(&self, ctx: &Ctx<'js>) -> Result<Inner<'js>> {
        let inner = self.stream.borrow().inner.clone();
        if !ReadableStream::reader_is_current(&inner, self.id) {
            return Err(Exception::throw_type(
                &ctx.clone(),
                "the reader was released",
            ));
        }
        Ok(inner)
    }
}

#[rquickjs::methods(rename_all = "camelCase")]
impl<'js> ReadableStreamDefaultReader<'js> {
    #[qjs(constructor)]
    pub fn new(ctx: Ctx<'js>, stream: Value<'js>) -> Result<Self> {
        let stream = stream
            .as_object()
            .and_then(Class::<ReadableStream>::from_object)
            .ok_or_else(|| Exception::throw_type(&ctx, "a ReadableStream is required"))?;
        let inner = stream.borrow().inner.clone();
        let id = ReadableStream::acquire_reader(&ctx, &inner)?;
        let closed = ReadableStream::closed_promise(&ctx, &inner)?;
        Ok(Self { stream, id, closed })
    }

    #[qjs(get)]
    pub fn closed(&self) -> Promise<'js> { self.closed.clone() }

    pub fn read(&self, ctx: Ctx<'js>) -> Result<Promise<'js>> {
        let inner = match self.inner(&ctx) {
            Ok(inner) => inner,
            Err(error) => return Cap::rejected(&ctx, thrown(&ctx, error)),
        };
        let cap = Cap::new(&ctx)?;
        let promise = cap.promise();
        ReadableStream::read(&ctx, &inner, ReadRequest::Js(cap));
        Ok(promise)
    }

    pub fn cancel(&self, ctx: Ctx<'js>, reason: Opt<Value<'js>>) -> Result<Promise<'js>> {
        let inner = match self.inner(&ctx) {
            Ok(inner) => inner,
            Err(error) => return Cap::rejected(&ctx, thrown(&ctx, error)),
        };
        ReadableStream::cancel_stream(
            &ctx,
            &inner,
            reason
                .0
                .unwrap_or_else(|| Value::new_undefined(ctx.clone())),
        )
    }

    pub fn release_lock(&self, ctx: Ctx<'js>) {
        let inner = self.stream.borrow().inner.clone();
        ReadableStream::release_reader(&ctx, &inner, self.id);
    }

    #[qjs(prop, rename = PredefinedAtom::SymbolToStringTag, configurable)]
    pub fn to_string_tag() -> &'static str { "ReadableStreamDefaultReader" }
}

pub use crate::streams::pipe::ReadableStreamAsyncIterator;
