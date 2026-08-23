//! Minimal WHATWG streams, enough for Blob.stream() and CompressionStream.

use std::{cell::RefCell, rc::Rc};

use den_util::{BufferSource, Probe as _};
use indexmap::indexmap;
use rquickjs::{
    ArrayBuffer, Class, Ctx, FromJs, Function, JsLifetime, Object, Promise, Result, TypedArray,
    Value,
    atom::PredefinedAtom,
    class::{Trace, Tracer},
    function::{Async, Opt, This},
    object::Accessor,
};

use crate::host::Host;

#[derive(Trace, JsLifetime)]
pub(crate) struct ReadableState<'js> {
    source:     Value<'js>,
    controller: Object<'js>,
    queue:      Vec<Value<'js>>,
    waiters:    Vec<Function<'js>>,
    errored:    Option<Value<'js>>,
    closed:          bool,
    locked:          bool,
    pulling:         bool,
    disturbed:       bool,
    closed_resolve:  Option<Function<'js>>,
    closed_reject:   Option<Function<'js>>,
}

#[derive(JsLifetime)]
#[rquickjs::class]
pub struct ReadableStream<'js> {
    pub(crate) state: Rc<RefCell<ReadableState<'js>>>,
}

impl<'js> Trace<'js> for ReadableStream<'js> {
    fn trace<'a>(&self, tracer: Tracer<'a, 'js>) {
        if let Ok(state) = self.state.try_borrow() {
            state.trace(tracer);
        }
    }
}

impl<'js> ReadableStream<'js> {
    fn wake(state: &mut ReadableState<'js>) {
        for resolve in state.waiters.drain(..) {
            let _ = resolve.call::<_, ()>(());
        }
        Self::settle_closed(state);
    }

    fn settle_closed(state: &mut ReadableState<'js>) {
        if let Some(error) = state.errored.clone() {
            if let Some(reject) = state.closed_reject.take() {
                let _ = reject.call::<_, ()>((error,));
            }
            state.closed_resolve = None;
        } else if state.closed {
            if let Some(resolve) = state.closed_resolve.take() {
                let _ = resolve.call::<_, ()>(());
            }
            state.closed_reject = None;
        }
    }

    fn controller_for(
        ctx: &Ctx<'js>, state: &Rc<RefCell<ReadableState<'js>>>,
    ) -> Result<Object<'js>> {
        let controller = Object::new(ctx.clone())?;
        let weak = Rc::downgrade(state);
        controller.set(
            "enqueue",
            Function::new(ctx.clone(), {
                let weak = weak.clone();
                move |chunk: Value<'js>| -> Result<()> {
                    let Some(state) = weak.upgrade() else {
                        return Ok(());
                    };
                    let mut state = state.borrow_mut();
                    if state.closed || state.errored.is_some() {
                        return Ok(());
                    }
                    state.queue.push(chunk);
                    ReadableStream::wake(&mut state);
                    Ok(())
                }
            })?,
        )?;
        controller.set(
            "close",
            Function::new(ctx.clone(), {
                let weak = weak.clone();
                move || -> Result<()> {
                    let Some(state) = weak.upgrade() else {
                        return Ok(());
                    };
                    let mut state = state.borrow_mut();
                    state.closed = true;
                    ReadableStream::wake(&mut state);
                    Ok(())
                }
            })?,
        )?;
        controller.set(
            "error",
            Function::new(ctx.clone(), {
                let weak = weak.clone();
                let ctx = ctx.clone();
                move |reason: Opt<Value<'js>>| -> Result<()> {
                    let Some(state) = weak.upgrade() else {
                        return Ok(());
                    };
                    let mut state = state.borrow_mut();
                    if state.errored.is_some() || state.closed {
                        return Ok(());
                    }
                    state.errored = Some(reason.0.unwrap_or_else(|| {
                        rquickjs::Exception::from_message(ctx.clone(), "ReadableStream errored")
                            .map(|exc| exc.into_object().into_value())
                            .unwrap_or_else(|_| Value::new_undefined(ctx.clone()))
                    }));
                    ReadableStream::wake(&mut state);
                    Ok(())
                }
            })?,
        )?;
        Ok(controller)
    }

    fn result(ctx: &Ctx<'js>, value: Value<'js>, done: bool) -> Result<Value<'js>> {
        indexmap! {
          "value" => value,
          "done" => done.into_js_bool(ctx)?,
        }
        .into_js_map(ctx)
    }

    async fn pull_if_empty(state: &Rc<RefCell<ReadableState<'js>>>, ctx: &Ctx<'js>) -> Result<()> {
        let (source, controller) = {
            let mut state = state.borrow_mut();
            if state.pulling || state.closed || state.errored.is_some() || !state.queue.is_empty() {
                return Ok(());
            }
            let source = state.source.clone();
            let Some(obj) = source.as_object() else {
                return Ok(());
            };
            if obj.get::<_, Function>("pull").is_err() {
                return Ok(());
            }
            state.pulling = true;
            (source, state.controller.clone())
        };
        let outcome = if let Some(obj) = source.as_object() {
            match obj.get::<_, Function>("pull") {
                Ok(pull) => pull.call::<_, Value>((This(obj.clone()), controller)),
                Err(_) => Ok(Value::new_undefined(ctx.clone())),
            }
        } else {
            Ok(Value::new_undefined(ctx.clone()))
        };
        match outcome {
            Ok(value) => {
                if let Err(error) = Host::maybe_await(value).await {
                    Self::error_with(state, ctx, error);
                }
            }
            Err(error) => Self::error_with(state, ctx, error),
        }
        state.borrow_mut().pulling = false;
        Ok(())
    }

    fn error_with(state: &Rc<RefCell<ReadableState<'js>>>, ctx: &Ctx<'js>, error: rquickjs::Error) {
        let reason = match error {
            rquickjs::Error::Exception => ctx.catch(),
            _ => {
                rquickjs::Exception::from_message(ctx.clone(), &error.to_string())
                    .map(|exc| exc.into_object().into_value())
                    .unwrap_or_else(|_| Value::new_undefined(ctx.clone()))
            }
        };
        let mut state = state.borrow_mut();
        state.errored = Some(reason);
        Self::wake(&mut state);
    }

    fn disturb_and_lock(stream: &Class<'js, Self>, ctx: Ctx<'js>) -> Result<()> {
        if stream.borrow().state.borrow().locked {
            stream.borrow().state.borrow_mut().disturbed = true;
            return Ok(());
        }
        Self::lock_for_consume(stream, &ctx)
    }

    pub fn lock_for_consume(stream: &Class<'js, Self>, ctx: &Ctx<'js>) -> Result<()> {
        let stream = stream.borrow();
        let mut state = stream.state.borrow_mut();
        if state.locked {
            return Err(Host::throw_type(ctx, "ReadableStream is locked"));
        }
        state.locked = true;
        state.disturbed = true;
        Ok(())
    }

    pub fn tee_pair(stream: &Class<'js, Self>, ctx: &Ctx<'js>) -> Result<(Value<'js>, Value<'js>)> {
        let queue = {
            let stream = stream.borrow();
            let mut state = stream.state.borrow_mut();
            if state.locked {
                return Err(Host::throw_type(ctx, "ReadableStream is locked"));
            }
            if state.errored.is_some() {
                return Err(Host::throw_type(ctx, "ReadableStream is errored"));
            }
            state.locked = true;
            state.disturbed = true;
            state.queue.clone()
        };
        let right = Self::from_queue(ctx, queue)?;
        Ok((stream.clone().into_value(), right.into_value()))
    }

    pub fn from_queue(ctx: &Ctx<'js>, queue: Vec<Value<'js>>) -> Result<Class<'js, Self>> {
        let state = Rc::new(RefCell::new(ReadableState {
            source:          Object::new(ctx.clone())?.into_value(),
            controller:      Object::new(ctx.clone())?,
            queue,
            waiters:         Vec::new(),
            errored:         None,
            closed:          true,
            locked:          false,
            pulling:         false,
            disturbed:       false,
            closed_resolve:  None,
            closed_reject:   None,
        }));
        Class::instance(ctx.clone(), Self { state })
    }

    fn fill_byob(
        ctx: &Ctx<'js>, result: Value<'js>, view: Option<Value<'js>>,
        state: &Rc<RefCell<ReadableState<'js>>>,
    ) -> Result<Value<'js>> {
        let Some(object) = result.as_object() else {
            return Ok(result);
        };
        if object.get::<_, bool>("done").unwrap_or(false) {
            return Ok(result);
        }
        let Some(view) = view else {
            return Ok(result);
        };
        let chunk: Value = object.get("value")?;
        let src = if let Ok(buffer) = ArrayBuffer::from_js(ctx, chunk.clone()) {
            buffer.as_bytes().unwrap_or(&[]).to_vec()
        } else if let Some(bytes) = ctx.probe(|| {
            BufferSource::is_array_buffer_view(ctx, &chunk)
                .ok()
                .filter(|is_view| *is_view)?;
            BufferSource::view_bytes(ctx, &chunk).ok()
        }) {
            bytes
        } else {
            return Ok(result);
        };
        let view_len: usize = view
            .as_object()
            .and_then(|object| object.get("byteLength").ok())
            .unwrap_or(0);
        let n = src.len().min(view_len);
        if src.len() > n {
            state
                .borrow_mut()
                .queue
                .insert(0, TypedArray::<u8>::new_copy(ctx.clone(), &src[n..])?.into_value());
        }
        ctx.globals().set("__denByobView", view)?;
        ctx.globals().set("__denByobResult", result)?;
        ctx.eval(
            r#"
              (function () {
                var result = globalThis.__denByobResult;
                var view = globalThis.__denByobView;
                delete globalThis.__denByobResult;
                delete globalThis.__denByobView;
                var chunk = result && result.value;
                if (!view || chunk == null) {
                  return result;
                }
                var src = chunk;
                if (!(src instanceof Uint8Array) && src && src.buffer) {
                  src = new Uint8Array(src.buffer, src.byteOffset || 0, src.byteLength || 0);
                }
                if (!(src instanceof Uint8Array)) {
                  return result;
                }
                var n = Math.min(view.byteLength, src.byteLength);
                new Uint8Array(view.buffer, view.byteOffset, n).set(src.subarray(0, n));
                return { done: false, value: new Uint8Array(view.buffer, view.byteOffset, n) };
              })()
            "#,
        )
    }

    pub async fn read_all_bytes(stream: &Class<'js, Self>, ctx: Ctx<'js>) -> Result<Vec<u8>> {
        let state = stream.borrow().state.clone();
        let mut out = Vec::new();
        loop {
            let result = Self::read_next(Rc::clone(&state), ctx.clone()).await?;
            let Some(object) = result.as_object() else {
                break;
            };
            if object.get::<_, bool>("done").unwrap_or(false) {
                break;
            }
            let value: Value = object.get("value")?;
            let Some(bytes) = ctx.probe(|| {
                BufferSource::is_array_buffer_view(&ctx, &value)
                    .ok()
                    .filter(|is_view| *is_view)?;
                BufferSource::view_bytes(&ctx, &value).ok()
            }) else {
                return Err(Host::throw_type(&ctx, "ReadableStream chunk must be a Uint8Array"));
            };
            out.extend(bytes);
        }
        Ok(out)
    }

    pub(crate) async fn read_next(
        state: Rc<RefCell<ReadableState<'js>>>, ctx: Ctx<'js>,
    ) -> Result<Value<'js>> {
        state.borrow_mut().disturbed = true;
        loop {
            {
                let mut state = state.borrow_mut();
                if let Some(error) = state.errored.clone() {
                    return Err(ctx.throw(error));
                }
                if !state.queue.is_empty() {
                    let value = state.queue.remove(0);
                    return Self::result(&ctx, value, false);
                }
                if state.closed {
                    return Self::result(&ctx, Value::new_undefined(ctx.clone()), true);
                }
            }
            Self::pull_if_empty(&state, &ctx).await?;
            {
                let state = state.borrow();
                if !state.queue.is_empty() || state.closed || state.errored.is_some() {
                    continue;
                }
            }
            let (promise, resolve, _reject) = ctx.promise()?;
            state.borrow_mut().waiters.push(resolve);
            let _ = promise.into_future::<Value>().await;
        }
    }
}

trait IntoJsBool<'js> {
    fn into_js_bool(self, ctx: &Ctx<'js>) -> Result<Value<'js>>;
}

impl<'js> IntoJsBool<'js> for bool {
    fn into_js_bool(self, ctx: &Ctx<'js>) -> Result<Value<'js>> {
        rquickjs::IntoJs::into_js(self, ctx)
    }
}

trait IntoJsMap<'js> {
    fn into_js_map(self, ctx: &Ctx<'js>) -> Result<Value<'js>>;
}

impl<'js> IntoJsMap<'js> for indexmap::IndexMap<&'static str, Value<'js>> {
    fn into_js_map(self, ctx: &Ctx<'js>) -> Result<Value<'js>> {
        rquickjs::IntoJs::into_js(self, ctx)
    }
}

#[rquickjs::methods(rename_all = "camelCase")]
impl<'js> ReadableStream<'js> {
    #[qjs(constructor)]
    pub fn new(ctx: Ctx<'js>, source: Opt<Value<'js>>) -> Result<Self> {
        let source = match source.0 {
            Some(value) if value.is_object() => value,
            _ => Object::new(ctx.clone())?.into_value(),
        };
        let placeholder = Object::new(ctx.clone())?;
        let state = Rc::new(RefCell::new(ReadableState {
            source:     source.clone(),
            controller: placeholder,
            queue:      Vec::new(),
            waiters:    Vec::new(),
            errored:    None,
            closed:          false,
            locked:          false,
            pulling:         false,
            disturbed:       false,
            closed_resolve:  None,
            closed_reject:   None,
        }));
        let controller = Self::controller_for(&ctx, &state)?;
        state.borrow_mut().controller = controller.clone();
        if let Some(obj) = source.as_object() {
            if let Ok(start) = obj.get::<_, Function>("start") {
                match start.call::<_, Value>((This(obj.clone()), controller)) {
                    Ok(value) => {
                        if value.is_promise() {
                            let state = Rc::clone(&state);
                            let ctx_err = ctx.clone();
                            ctx.spawn(async move {
                                if let Err(error) = Host::maybe_await(value).await {
                                    ReadableStream::error_with(&state, &ctx_err, error);
                                }
                            });
                        }
                    }
                    Err(error) => Self::error_with(&state, &ctx, error),
                }
            }
        }
        Ok(Self { state })
    }

    #[qjs(get)]
    pub fn locked(&self) -> bool { self.state.borrow().locked }

    pub fn is_disturbed(&self) -> bool { self.state.borrow().disturbed }

    #[qjs(get, rename = "_denDisturbed")]
    pub fn den_disturbed(&self) -> bool { self.is_disturbed() }

    pub fn get_reader(&self, ctx: Ctx<'js>, options: Opt<Value<'js>>) -> Result<Object<'js>> {
        if self.state.borrow().locked {
            return Err(Host::throw_type(&ctx, "ReadableStream is locked"));
        }
        self.state.borrow_mut().locked = true;
        let byob = options
            .0
            .as_ref()
            .and_then(|value| value.as_object())
            .and_then(|object| object.get::<_, String>("mode").ok())
            .is_some_and(|mode| mode == "byob");
        let reader = Object::new(ctx.clone())?;
        let state = Rc::clone(&self.state);
        reader.prop(
            "closed",
            Accessor::from({
                let state = Rc::clone(&state);
                move |this: This<Object<'js>>, ctx: Ctx<'js>| -> Result<Promise<'js>> {
                    if let Ok(existing) = this.0.get::<_, Promise<'_>>("_denClosed") {
                        return Ok(existing);
                    }
                    let (closed, resolve, reject) = ctx.promise()?;
                    {
                        let mut state = state.borrow_mut();
                        if let Some(error) = state.errored.clone() {
                            let _ = reject.call::<_, ()>((error,));
                        } else if state.closed {
                            let _ = resolve.call::<_, ()>(());
                        } else {
                            state.closed_resolve = Some(resolve);
                            state.closed_reject = Some(reject);
                        }
                    }
                    this.0.set("_denClosed", closed.clone())?;
                    Ok(closed)
                }
            })
            .enumerable(),
        )?;
        reader.set(
            "read",
            Function::new(ctx.clone(), {
                let state = Rc::clone(&state);
                move |ctx: Ctx<'js>, _this: This<Value<'js>>, view: Opt<Value<'js>>| -> Result<Promise<'js>> {
                    state.borrow_mut().disturbed = true;
                    let (promise, resolve, reject) = ctx.promise()?;
                    let state = Rc::clone(&state);
                    let ctx_err = ctx.clone();
                    let view = view.0;
                    ctx.spawn(async move {
                        match ReadableStream::read_next(Rc::clone(&state), ctx_err.clone()).await {
                            Ok(value) => {
                                let value = if byob {
                                    match ReadableStream::fill_byob(&ctx_err, value, view, &state) {
                                        Ok(value) => value,
                                        Err(_) => {
                                            let thrown = ctx_err.catch();
                                            let _ = reject.call::<_, ()>((thrown,));
                                            return;
                                        }
                                    }
                                } else {
                                    value
                                };
                                let _ = resolve.call::<_, ()>((value,));
                            }
                            Err(_) => {
                                let thrown = ctx_err.catch();
                                let _ = reject.call::<_, ()>((thrown,));
                            }
                        }
                    });
                    Ok(promise)
                }
            })?,
        )?;
        reader.set(
            "cancel",
            Function::new(
                ctx.clone(),
                Async({
                    let state = Rc::clone(&state);
                    move |ctx: Ctx<'js>, reason: Opt<Value<'js>>| {
                        let state = Rc::clone(&state);
                        async move {
                            {
                                let mut state = state.borrow_mut();
                                state.disturbed = true;
                            }
                            let source = state.borrow().source.clone();
                            if let Some(obj) = source.as_object() {
                                if let Ok(cancel) = obj.get::<_, Function>("cancel") {
                                    let _ = cancel.call::<_, Value>((This(obj.clone()), reason.0));
                                }
                            }
                            let mut state = state.borrow_mut();
                            state.queue.clear();
                            state.closed = true;
                            state.locked = false;
                            ReadableStream::wake(&mut state);
                            Ok::<Value<'js>, rquickjs::Error>(Value::new_undefined(ctx.clone()))
                        }
                    }
                }),
            )?,
        )?;
        reader.set(
            "releaseLock",
            Function::new(ctx.clone(), {
                let state = Rc::clone(&state);
                move || -> Result<()> {
                    state.borrow_mut().locked = false;
                    Ok(())
                }
            })?,
        )?;
        Ok(reader)
    }

    pub fn pipe_through(
        this: This<Class<'js, Self>>, ctx: Ctx<'js>, transform: Object<'js>,
    ) -> Result<Value<'js>> {
        let readable: Value<'js> = transform.get("readable")?;
        let writable: Value<'js> = transform.get("writable")?;
        if readable.is_undefined() || writable.is_undefined() {
            return Err(Host::throw_type(
                &ctx,
                "pipeThrough requires a { readable, writable } pair",
            ));
        }
        if writable.as_object().is_none() {
            return Err(Host::throw_type(
                &ctx,
                "pipeThrough requires a { readable, writable } pair",
            ));
        }
        Self::disturb_and_lock(&this.0, ctx)?;
        Ok(readable)
    }

    pub fn pipe_to(
        this: This<Class<'js, Self>>, ctx: Ctx<'js>, dest: Opt<Value<'js>>,
    ) -> Result<Promise<'js>> {
        Self::disturb_and_lock(&this.0, ctx.clone())?;
        let (promise, resolve, reject) = ctx.promise()?;
        let stream = this.0;
        let dest = dest.0;
        let ctx_err = ctx.clone();
        ctx.spawn(async move {
            match ReadableStream::read_all_bytes(&stream, ctx_err.clone()).await {
                Ok(bytes) => {
                    let written = if let Some(dest) = dest {
                        if let Some(object) = dest.as_object()
                            && let Some(writable) = Class::<WritableStream>::from_object(object)
                        {
                            let sink = writable.borrow().state.borrow().sink.clone();
                            if let Some(sink) = sink.as_object()
                                && let Ok(write) = sink.get::<_, Function>("write")
                            {
                                match rquickjs::TypedArray::<u8>::new_copy(
                                    ctx_err.clone(),
                                    bytes,
                                ) {
                                    Ok(chunk) => write
                                        .call::<_, Value>((This(sink.clone()), chunk))
                                        .map(|_| ()),
                                    Err(error) => Err(error),
                                }
                            } else {
                                Ok(())
                            }
                        } else {
                            Ok(())
                        }
                    } else {
                        Ok(())
                    };
                    match written {
                        Ok(()) => {
                            let _ = resolve.call::<_, ()>(());
                        }
                        Err(_) => {
                            let thrown = ctx_err.catch();
                            let _ = reject.call::<_, ()>((thrown,));
                        }
                    }
                }
                Err(_) => {
                    let thrown = ctx_err.catch();
                    let _ = reject.call::<_, ()>((thrown,));
                }
            }
        });
        Ok(promise)
    }

    pub fn cancel(
        this: This<Class<'js, Self>>, ctx: Ctx<'js>, reason: Opt<Value<'js>>,
    ) -> Result<Promise<'js>> {
        let source = {
            let stream = this.0.borrow();
            let mut state = stream.state.borrow_mut();
            state.disturbed = true;
            state.closed = true;
            state.queue.clear();
            let source = state.source.clone();
            ReadableStream::wake(&mut state);
            source
        };
        if let Some(object) = source.as_object()
            && let Ok(cancel) = object.get::<_, Function>("cancel")
        {
            let _ = cancel.call::<_, Value>((This(object.clone()), reason.0));
        }
        let (promise, resolve, _reject) = ctx.promise()?;
        let _ = resolve.call::<_, ()>(());
        Ok(promise)
    }

    #[qjs(rename = "_denAbort")]
    pub fn den_abort(&self, reason: Opt<Value<'js>>) {
        let mut state = self.state.borrow_mut();
        if state.closed || state.errored.is_some() {
            return;
        }
        state.errored = reason.0;
        Self::wake(&mut state);
    }

    #[qjs(prop, rename = PredefinedAtom::SymbolToStringTag, configurable)]
    pub fn to_string_tag() -> &'static str { "ReadableStream" }
}

#[derive(Trace, JsLifetime)]
struct WritableState<'js> {
    sink:    Value<'js>,
    closed:  bool,
    errored: Option<Value<'js>>,
}

#[derive(JsLifetime)]
#[rquickjs::class]
pub struct WritableStream<'js> {
    state: Rc<RefCell<WritableState<'js>>>,
}

impl<'js> Trace<'js> for WritableStream<'js> {
    fn trace<'a>(&self, tracer: Tracer<'a, 'js>) {
        if let Ok(state) = self.state.try_borrow() {
            state.trace(tracer);
        }
    }
}

#[rquickjs::methods(rename_all = "camelCase")]
impl<'js> WritableStream<'js> {
    #[qjs(constructor)]
    pub fn new(ctx: Ctx<'js>, sink: Opt<Value<'js>>) -> Result<Self> {
        let sink = match sink.0 {
            Some(value) if value.is_object() => value,
            _ => Object::new(ctx.clone())?.into_value(),
        };
        let state = Rc::new(RefCell::new(WritableState {
            sink:    sink.clone(),
            closed:  false,
            errored: None,
        }));
        if let Some(obj) = sink.as_object() {
            if let Ok(start) = obj.get::<_, Function>("start") {
                if let Err(error) = start.call::<_, Value>((This(obj.clone()),)) {
                    let reason = match error {
                        rquickjs::Error::Exception => ctx.catch(),
                        _ => Value::new_undefined(ctx.clone()),
                    };
                    state.borrow_mut().errored = Some(reason);
                }
            }
        }
        Ok(Self { state })
    }

    pub fn get_writer(&self, ctx: Ctx<'js>) -> Result<Object<'js>> {
        let writer = Object::new(ctx.clone())?;
        let state = Rc::clone(&self.state);
        writer.set(
            "write",
            Function::new(ctx.clone(), {
                let state = Rc::clone(&state);
                move |ctx: Ctx<'js>, chunk: Value<'js>| -> Result<Value<'js>> {
                    let (errored, closed, sink) = {
                        let state = state.borrow();
                        (state.errored.clone(), state.closed, state.sink.clone())
                    };
                    if let Some(error) = errored {
                        return Err(ctx.throw(error));
                    }
                    if closed {
                        return Err(Host::throw_type(&ctx, "WritableStream is closed"));
                    }
                    let outcome = if let Some(obj) = sink.as_object() {
                        match obj.get::<_, Function>("write") {
                            Ok(write) => write.call::<_, Value>((This(obj.clone()), chunk)),
                            Err(_) => Ok(Value::new_undefined(ctx.clone())),
                        }
                    } else {
                        Ok(Value::new_undefined(ctx.clone()))
                    };
                    match outcome {
                        Ok(value) => Ok(value),
                        Err(error) => {
                            let reason = match error {
                                rquickjs::Error::Exception => ctx.catch(),
                                _ => Value::new_undefined(ctx.clone()),
                            };
                            state.borrow_mut().errored = Some(reason.clone());
                            Err(ctx.throw(reason))
                        }
                    }
                }
            })?,
        )?;
        writer.set(
            "close",
            Function::new(ctx.clone(), {
                let state = Rc::clone(&state);
                move |ctx: Ctx<'js>| -> Result<Value<'js>> {
                    let sink = {
                        let mut state = state.borrow_mut();
                        if let Some(error) = state.errored.clone() {
                            return Err(ctx.throw(error));
                        }
                        state.closed = true;
                        state.sink.clone()
                    };
                    let outcome = if let Some(obj) = sink.as_object() {
                        match obj.get::<_, Function>("close") {
                            Ok(close) => close.call::<_, Value>((This(obj.clone()),)),
                            Err(_) => Ok(Value::new_undefined(ctx.clone())),
                        }
                    } else {
                        Ok(Value::new_undefined(ctx.clone()))
                    };
                    match outcome {
                        Ok(value) => Ok(value),
                        Err(error) => {
                            let reason = match error {
                                rquickjs::Error::Exception => ctx.catch(),
                                _ => Value::new_undefined(ctx.clone()),
                            };
                            state.borrow_mut().errored = Some(reason.clone());
                            Err(ctx.throw(reason))
                        }
                    }
                }
            })?,
        )?;
        writer.set(
            "abort",
            Function::new(ctx.clone(), {
                let state = Rc::clone(&state);
                move |ctx: Ctx<'js>, reason: Opt<Value<'js>>| -> Result<Value<'js>> {
                    state.borrow_mut().closed = true;
                    let sink = state.borrow().sink.clone();
                    if let Some(obj) = sink.as_object() {
                        if let Ok(abort) = obj.get::<_, Function>("abort") {
                            return abort.call((This(obj.clone()), reason.0));
                        }
                    }
                    Ok(Value::new_undefined(ctx.clone()))
                }
            })?,
        )?;
        Ok(writer)
    }

    #[qjs(prop, rename = PredefinedAtom::SymbolToStringTag, configurable)]
    pub fn to_string_tag() -> &'static str { "WritableStream" }
}

#[derive(Trace, JsLifetime)]
#[rquickjs::class]
pub struct TransformStream<'js> {
    readable: Class<'js, ReadableStream<'js>>,
    writable: Class<'js, WritableStream<'js>>,
}

#[rquickjs::methods]
impl<'js> TransformStream<'js> {
    #[qjs(constructor)]
    pub fn new(ctx: Ctx<'js>, transformer: Opt<Value<'js>>) -> Result<Self> {
        let transformer = match transformer.0.and_then(Value::into_object) {
            Some(object) => object,
            None => Object::new(ctx.clone())?,
        };
        let readable = Class::instance(ctx.clone(), ReadableStream::new(ctx.clone(), Opt(None))?)?;
        let controller = readable.borrow().state.borrow().controller.clone();
        let write_sink = Object::new(ctx.clone())?;
        write_sink.set(
            "write",
            Function::new(ctx.clone(), {
                let transformer = transformer.clone();
                let controller = controller.clone();
                move |chunk: Value<'js>| -> Result<Value<'js>> {
                    match transformer.get::<_, Function>("transform") {
                        Ok(transform) => {
                            transform.call((This(transformer.clone()), chunk, controller.clone()))
                        }
                        Err(_) => Ok(Value::new_undefined(transformer.ctx().clone())),
                    }
                }
            })?,
        )?;
        write_sink.set(
            "close",
            Function::new(ctx.clone(), {
                let transformer = transformer.clone();
                let controller = controller.clone();
                move |ctx: Ctx<'js>| -> Result<Value<'js>> {
                    let flushed = match transformer.get::<_, Function>("flush") {
                        Ok(flush) => {
                            flush
                                .call::<_, Value>((This(transformer.clone()), controller.clone()))?
                        }
                        Err(_) => Value::new_undefined(ctx.clone()),
                    };
                    let close: Function = controller.get("close")?;
                    if flushed.is_promise() {
                        let controller = controller.clone();
                        ctx.spawn(async move {
                            let _ = Host::maybe_await(flushed).await;
                            let _ = close.call::<_, ()>((This(controller),));
                        });
                        return Ok(Value::new_undefined(ctx.clone()));
                    }
                    close.call::<_, ()>((This(controller.clone()),))?;
                    Ok(flushed)
                }
            })?,
        )?;
        write_sink.set(
            "abort",
            Function::new(ctx.clone(), {
                let controller = controller.clone();
                move |reason: Opt<Value<'js>>| -> Result<()> {
                    let error: Function = controller.get("error")?;
                    error.call::<_, ()>((This(controller.clone()), reason.0))?;
                    Ok(())
                }
            })?,
        )?;
        let writable = Class::instance(
            ctx.clone(),
            WritableStream::new(ctx.clone(), Opt(Some(write_sink.into_value())))?,
        )?;
        Ok(Self { readable, writable })
    }

    #[qjs(get)]
    pub fn readable(&self) -> Class<'js, ReadableStream<'js>> { self.readable.clone() }

    #[qjs(get)]
    pub fn writable(&self) -> Class<'js, WritableStream<'js>> { self.writable.clone() }

    #[qjs(prop, rename = PredefinedAtom::SymbolToStringTag, configurable)]
    pub fn to_string_tag() -> &'static str { "TransformStream" }
}
