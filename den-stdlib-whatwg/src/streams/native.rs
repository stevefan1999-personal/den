//! The seam a Rust byte producer or consumer plugs into.
//!
//! `pull` is invoked only while the stream has demand, so the high-water mark
//! is the real backpressure knob, and this module is the only place in the
//! subsystem that reaches for `ctx.spawn`.

use std::{cell::RefCell, fmt, future::Future, pin::Pin, rc::Rc};

use rquickjs::{Class, Ctx, Result, TypedArray, Value};

use crate::streams::{
    Cap,
    readable::{Inner as RsInner, ReadableStream},
    thrown, type_error,
    writable::{Inner as WsInner, WritableStream},
};

pub type PullFuture<'js> =
    Pin<Box<dyn Future<Output = std::result::Result<Option<Vec<u8>>, StreamError>> + 'js>>;
pub type SinkFuture<'js> =
    Pin<Box<dyn Future<Output = std::result::Result<(), StreamError>> + 'js>>;

#[derive(Debug)]
pub enum StreamError {
    Io(std::io::Error),
    Message(String),
}

impl fmt::Display for StreamError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Io(error) => write!(formatter, "{error}"),
            Self::Message(message) => formatter.write_str(message),
        }
    }
}

impl From<std::io::Error> for StreamError {
    fn from(error: std::io::Error) -> Self { Self::Io(error) }
}

/// A host byte source. `Ok(None)` ends the stream.
pub struct ByteSource<'js> {
    pull:   Box<dyn FnMut(Ctx<'js>) -> PullFuture<'js> + 'js>,
    cancel: Option<Box<dyn FnOnce(Value<'js>) + 'js>>,
}

// SAFETY: the boxed closures are `'js`-scoped and hold no other lifetime.
unsafe impl<'js> rquickjs::JsLifetime<'js> for ByteSource<'js> {
    type Changed<'to> = ByteSource<'to>;
}

pub(crate) type NativeSource<'js> = ByteSource<'js>;

impl<'js> ByteSource<'js> {
    pub(crate) fn cancel(&mut self, reason: Value<'js>) {
        if let Some(cancel) = self.cancel.take() {
            cancel(reason);
        }
    }
}

pub struct ByteSink<'js> {
    write: Box<dyn FnMut(Ctx<'js>, Vec<u8>) -> SinkFuture<'js> + 'js>,
    close: Option<Box<dyn FnOnce(Ctx<'js>) -> SinkFuture<'js> + 'js>>,
    abort: Option<Box<dyn FnOnce(Value<'js>) + 'js>>,
}

// SAFETY: see `ByteSource`.
unsafe impl<'js> rquickjs::JsLifetime<'js> for ByteSink<'js> {
    type Changed<'to> = ByteSink<'to>;
}

pub(crate) type NativeSink<'js> = ByteSink<'js>;

impl<'js> ByteSink<'js> {
    pub(crate) fn abort(&mut self, reason: Value<'js>) {
        if let Some(abort) = self.abort.take() {
            abort(reason);
        }
    }
}

impl<'js> ReadableStream<'js> {
    /// Build a stream over a Rust byte source. `hwm` counts chunks: 1 paces the
    /// producer to one chunk in flight, which is what fetch and den:http want.
    pub fn from_native(
        ctx: &Ctx<'js>, hwm: f64, pull: impl FnMut(Ctx<'js>) -> PullFuture<'js> + 'js,
        cancel: impl FnOnce(Value<'js>) + 'js,
    ) -> Result<Class<'js, Self>> {
        let inner = Self::new_inner(ctx)?;
        {
            let mut borrow = inner.borrow_mut();
            borrow.started = true;
            borrow.hwm = hwm;
            borrow.native = Some(Rc::new(RefCell::new(ByteSource {
                pull:   Box::new(pull),
                cancel: Some(Box::new(cancel)),
            })));
        }
        Self::attach_controller(ctx, &inner)?;
        let stream = Class::instance(ctx.clone(), Self {
            inner: inner.clone(),
        })?;
        Self::pull_if_needed(ctx, &inner);
        Ok(stream)
    }
}

impl<'js> WritableStream<'js> {
    pub fn to_native(
        ctx: &Ctx<'js>, hwm: f64, write: impl FnMut(Ctx<'js>, Vec<u8>) -> SinkFuture<'js> + 'js,
        close: impl FnOnce(Ctx<'js>) -> SinkFuture<'js> + 'js,
        abort: impl FnOnce(Value<'js>) + 'js,
    ) -> Result<Class<'js, Self>> {
        let inner = Self::new_inner(ctx)?;
        {
            let mut borrow = inner.borrow_mut();
            borrow.started = true;
            borrow.hwm = hwm;
            borrow.native = Some(Rc::new(RefCell::new(ByteSink {
                write: Box::new(write),
                close: Some(Box::new(close)),
                abort: Some(Box::new(abort)),
            })));
        }
        Self::attach_controller(ctx, &inner)?;
        Class::instance(ctx.clone(), Self { inner })
    }
}

/// One spawned future per outstanding pull — never a loop, so `idle()` still
/// resolves and cancelling the stream discards an in-flight result.
pub(crate) fn drive_pull<'js>(ctx: &Ctx<'js>, inner: &RsInner<'js>) {
    let Some(source) = inner.borrow().native.clone() else {
        ReadableStream::pull_settled(ctx, inner);
        return;
    };
    let future = (source.borrow_mut().pull)(ctx.clone());
    // Weak: an in-flight pull must not keep a dropped stream alive, and a
    // second strong owner would break the single-tracer rule.
    let inner = Rc::downgrade(inner);
    let spawn_ctx = ctx.clone();
    ctx.spawn(async move {
        let outcome = future.await;
        let Some(inner) = inner.upgrade() else {
            return;
        };
        match outcome {
            Ok(Some(bytes)) => {
                match TypedArray::<u8>::new_copy(spawn_ctx.clone(), bytes) {
                    Ok(chunk) => {
                        let _ = ReadableStream::enqueue(&spawn_ctx, &inner, chunk.into_value());
                    }
                    Err(error) => {
                        let reason = thrown(&spawn_ctx, error);
                        ReadableStream::error(&spawn_ctx, &inner, reason);
                    }
                }
            }
            Ok(None) => {
                let _ = ReadableStream::close_requested(&spawn_ctx, &inner);
            }
            Err(error) => {
                let reason = type_error(&spawn_ctx, &error.to_string());
                ReadableStream::error(&spawn_ctx, &inner, reason);
            }
        }
        ReadableStream::pull_settled(&spawn_ctx, &inner);
    });
}

pub(crate) fn drive_write<'js>(
    ctx: &Ctx<'js>, inner: &WsInner<'js>, sink: &Rc<RefCell<ByteSink<'js>>>, chunk: Value<'js>,
) -> Result<Value<'js>> {
    let Some(bytes) = crate::host::Host::buffer_source_bytes(ctx, chunk)? else {
        return Err(rquickjs::Exception::throw_type(
            ctx,
            "a native sink only accepts Uint8Array chunks",
        ));
    };
    let future = (sink.borrow_mut().write)(ctx.clone(), bytes);
    settle_native(ctx, inner, future)
}

pub(crate) fn drive_close<'js>(
    ctx: &Ctx<'js>, inner: &WsInner<'js>, sink: &Rc<RefCell<ByteSink<'js>>>,
) -> Result<Value<'js>> {
    let Some(close) = sink.borrow_mut().close.take() else {
        return Ok(Value::new_undefined(ctx.clone()));
    };
    let future = close(ctx.clone());
    settle_native(ctx, inner, future)
}

fn settle_native<'js>(
    ctx: &Ctx<'js>, _inner: &WsInner<'js>, future: SinkFuture<'js>,
) -> Result<Value<'js>> {
    let cap = Rc::new(RefCell::new(Cap::new(ctx)?));
    let promise = cap.borrow().promise();
    let spawn_ctx = ctx.clone();
    ctx.spawn(async move {
        match future.await {
            Ok(()) => cap.borrow_mut().fulfill(&spawn_ctx),
            Err(error) => {
                let reason = type_error(&spawn_ctx, &error.to_string());
                cap.borrow_mut().reject(reason);
            }
        }
    });
    Ok(promise.into_value())
}
