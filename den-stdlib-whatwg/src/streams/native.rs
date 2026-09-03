//! The seam a Rust byte consumer plugs into.
//!
//! Writes are handed to the sink one at a time, so the high-water mark is the
//! real backpressure knob, and this module is the only place in the subsystem
//! that reaches for `ctx.spawn`.

use std::{cell::RefCell, future::Future, pin::Pin, rc::Rc};

use rquickjs::{Class, Ctx, Result, Value};

use crate::streams::{Cap, WritableStream, type_error};

pub type SinkFuture<'js> = Pin<Box<dyn Future<Output = std::result::Result<(), String>> + 'js>>;

pub struct ByteSink<'js> {
    write: Box<dyn FnMut(Ctx<'js>, Vec<u8>) -> SinkFuture<'js> + 'js>,
    close: Option<Box<dyn FnOnce(Ctx<'js>) -> SinkFuture<'js> + 'js>>,
    abort: Option<Box<dyn FnOnce(Value<'js>) + 'js>>,
}

// SAFETY: the boxed closures are `'js`-scoped and hold no other lifetime.
unsafe impl<'js> rquickjs::JsLifetime<'js> for ByteSink<'js> {
    type Changed<'to> = ByteSink<'to>;
}

impl<'js> ByteSink<'js> {
    pub(crate) fn abort(&mut self, reason: Value<'js>) {
        if let Some(abort) = self.abort.take() {
            abort(reason);
        }
    }
}

impl<'js> WritableStream<'js> {
    pub fn to_native<W, C, A>(
        ctx: &Ctx<'js>, hwm: f64, write: W, close: C, abort: A,
    ) -> Result<Class<'js, Self>>
    where
        W: FnMut(Ctx<'js>, Vec<u8>) -> SinkFuture<'js> + 'js,
        C: FnOnce(Ctx<'js>) -> SinkFuture<'js> + 'js,
        A: FnOnce(Value<'js>) + 'js,
    {
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
        Self::wrap(ctx, inner)
    }
}

pub(crate) fn drive_write<'js>(
    ctx: &Ctx<'js>, sink: &Rc<RefCell<ByteSink<'js>>>, chunk: Value<'js>,
) -> Result<Value<'js>> {
    let Some(bytes) = crate::host::Host::buffer_source_bytes(ctx, chunk)? else {
        return Err(rquickjs::Exception::throw_type(
            ctx,
            "a native sink only accepts Uint8Array chunks",
        ));
    };
    let future = (sink.borrow_mut().write)(ctx.clone(), bytes);
    settle_native(ctx, future)
}

pub(crate) fn drive_close<'js>(
    ctx: &Ctx<'js>, sink: &Rc<RefCell<ByteSink<'js>>>,
) -> Result<Value<'js>> {
    let Some(close) = sink.borrow_mut().close.take() else {
        return Ok(Value::new_undefined(ctx.clone()));
    };
    let future = close(ctx.clone());
    settle_native(ctx, future)
}

fn settle_native<'js>(ctx: &Ctx<'js>, future: SinkFuture<'js>) -> Result<Value<'js>> {
    let mut cap = Cap::new(ctx)?;
    let promise = cap.promise();
    let spawn_ctx = ctx.clone();
    ctx.spawn(async move {
        match future.await {
            Ok(()) => cap.fulfill(&spawn_ctx),
            Err(error) => {
                let reason = type_error(&spawn_ctx, &error);
                cap.reject(reason);
            }
        }
    });
    Ok(promise.into_value())
}
