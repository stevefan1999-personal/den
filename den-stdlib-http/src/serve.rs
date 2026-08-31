use std::{
    cell::Cell,
    io,
    net::{SocketAddr, TcpListener},
    rc::Rc,
    time::Duration,
};

use rquickjs::{
    Ctx, Exception, FromJs, Function, JsLifetime, Object, Promise, Result, Value, class::Trace,
    function::Opt,
};
use tokio::{
    sync::{mpsc, watch},
    time::Instant,
};

use crate::{
    conn::Connection,
    dispatch::Dispatch,
    error::{HttpError, HttpErrorKind},
};

#[derive(Clone, Copy, Eq, PartialEq)]
pub enum Phase {
    Serving,
    Draining {
        deadline: Option<Instant>,
        abort:    bool,
    },
    Done,
}

impl Phase {
    pub const fn is_draining(self) -> bool { matches!(self, Self::Draining { .. }) }

    pub const fn is_aborting(self) -> bool {
        matches!(self, Self::Draining { abort: true, .. } | Self::Done)
    }
}

pub struct ServeOptions<'js> {
    fetch:  Function<'js>,
    listen: Listen,
}

struct Listen {
    host: String,
    port: u16,
}

impl Default for Listen {
    fn default() -> Self {
        Self {
            host: "127.0.0.1".to_string(),
            port: 8000,
        }
    }
}

impl<'js> FromJs<'js> for ServeOptions<'js> {
    fn from_js(ctx: &Ctx<'js>, value: Value<'js>) -> Result<Self> {
        let options = Object::from_js(ctx, value)
            .map_err(|_error| Exception::throw_type(ctx, "serve options must be an object"))?;
        let fetch = options
            .get::<_, Option<Function>>("fetch")?
            .ok_or_else(|| Exception::throw_type(ctx, "serve requires a fetch handler"))?;
        let listen = options
            .get::<_, Option<Object>>("listen")?
            .map(Listen::from_object)
            .transpose()?
            .unwrap_or_default();
        Ok(Self { fetch, listen })
    }
}

impl Listen {
    fn from_object(object: Object<'_>) -> Result<Self> {
        Ok(Self {
            host: object
                .get::<_, Option<String>>("host")?
                .unwrap_or_else(|| "127.0.0.1".to_string()),
            port: object.get::<_, Option<u16>>("port")?.unwrap_or(8000),
        })
    }

    fn bind(&self, ctx: &Ctx<'_>) -> Result<(tokio::net::TcpListener, SocketAddr)> {
        let listener = TcpListener::bind((self.host.as_str(), self.port)).map_err(|error| {
            let kind = if error.kind() == io::ErrorKind::AddrInUse {
                HttpErrorKind::AddrInUse
            } else {
                HttpErrorKind::Bind
            };
            HttpError::throw(ctx, kind, error.to_string())
        })?;
        let addr = listener
            .local_addr()
            .map_err(|error| HttpError::throw(ctx, HttpErrorKind::Bind, error.to_string()))?;
        listener
            .set_nonblocking(true)
            .map_err(|error| HttpError::throw(ctx, HttpErrorKind::Bind, error.to_string()))?;
        let listener = tokio::net::TcpListener::from_std(listener)
            .map_err(|error| HttpError::throw(ctx, HttpErrorKind::Bind, error.to_string()))?;
        Ok((listener, addr))
    }
}

#[derive(Default)]
pub struct Counters {
    requests:    Cell<usize>,
    connections: Cell<usize>,
}

impl Counters {
    pub fn request(self: &Rc<Self>) -> PendingGuard {
        self.requests.set(self.requests.get().saturating_add(1));
        PendingGuard {
            counters: Rc::clone(self),
            request:  true,
        }
    }

    fn connection(self: &Rc<Self>) -> PendingGuard {
        self.connections
            .set(self.connections.get().saturating_add(1));
        PendingGuard {
            counters: Rc::clone(self),
            request:  false,
        }
    }
}

pub struct PendingGuard {
    counters: Rc<Counters>,
    request:  bool,
}

impl Drop for PendingGuard {
    fn drop(&mut self) {
        let counter = if self.request {
            &self.counters.requests
        } else {
            &self.counters.connections
        };
        counter.set(counter.get().saturating_sub(1));
    }
}

#[derive(Clone, Trace)]
#[rquickjs::class(rename = "Server")]
pub struct Server {
    #[qjs(skip_trace)]
    addr:     SocketAddr,
    #[qjs(skip_trace)]
    url:      String,
    #[qjs(skip_trace)]
    phase:    watch::Sender<Phase>,
    #[qjs(skip_trace)]
    counters: Rc<Counters>,
}

unsafe impl JsLifetime<'_> for Server {
    type Changed<'to> = Server;
}

#[rquickjs::methods]
impl Server {
    #[qjs(constructor)]
    pub fn new(ctx: Ctx<'_>) -> Result<Self> {
        Err(Exception::throw_type(&ctx, "Illegal constructor"))
    }

    #[qjs(get, enumerable)]
    pub fn addr<'js>(&self, ctx: Ctx<'js>) -> Result<Object<'js>> { socket_addr(&ctx, self.addr) }

    #[qjs(get, enumerable)]
    pub fn url(&self) -> String { self.url.clone() }

    #[qjs(get, enumerable)]
    pub fn pending<'js>(&self, ctx: Ctx<'js>) -> Result<Object<'js>> {
        let pending = Object::new(ctx)?;
        pending.set("requests", self.counters.requests.get())?;
        pending.set("connections", self.counters.connections.get())?;
        Ok(pending)
    }

    #[qjs(get, enumerable)]
    pub fn finished<'js>(&self, ctx: Ctx<'js>) -> Result<Promise<'js>> {
        Self::finished_promise(ctx, self.phase.subscribe())
    }

    pub fn close<'js>(&self, ctx: Ctx<'js>, options: Opt<Value<'js>>) -> Result<Promise<'js>> {
        let options = options
            .0
            .filter(|value| !value.is_null() && !value.is_undefined());
        let deadline = match options {
            None => None,
            Some(value) => {
                let options = Object::from_js(&ctx, value).map_err(|_error| {
                    Exception::throw_type(&ctx, "Server.close options must be an object")
                })?;
                let drain_ms: Value = options.get("drainMs")?;
                if drain_ms.is_undefined() {
                    None
                } else {
                    let drain_ms = f64::from_js(&ctx, drain_ms)?;
                    let duration =
                        Duration::try_from_secs_f64(drain_ms / 1_000.0).map_err(|_error| {
                            Exception::throw_range(
                                &ctx,
                                "drainMs must be a finite non-negative number",
                            )
                        })?;
                    Some(
                        Instant::now()
                            .checked_add(duration)
                            .ok_or_else(|| Exception::throw_range(&ctx, "drainMs is too large"))?,
                    )
                }
            }
        };
        let now = Instant::now();
        self.phase.send_if_modified(|phase| {
            let next = match *phase {
                Phase::Serving => {
                    Phase::Draining {
                        deadline,
                        abort: deadline.is_some_and(|deadline| deadline <= now),
                    }
                }
                Phase::Draining {
                    deadline: current,
                    abort,
                } => {
                    let deadline = match (current, deadline) {
                        (Some(current), Some(next)) => Some(current.min(next)),
                        (None, Some(next)) => Some(next),
                        (current, None) => current,
                    };
                    Phase::Draining {
                        deadline,
                        abort: abort || deadline.is_some_and(|deadline| deadline <= now),
                    }
                }
                Phase::Done => Phase::Done,
            };
            if next == *phase {
                false
            } else {
                *phase = next;
                true
            }
        });
        Self::finished_promise(ctx, self.phase.subscribe())
    }
}

impl Server {
    fn finished_promise(ctx: Ctx<'_>, mut phase: watch::Receiver<Phase>) -> Result<Promise<'_>> {
        let (promise, resolve, reject) = ctx.promise()?;
        let task_ctx = ctx.clone();
        ctx.spawn(async move {
            let result = phase.wait_for(|phase| *phase == Phase::Done).await;
            if result.is_ok() {
                let _ = resolve.call::<_, ()>(((),));
            } else {
                let error = HttpError::instance(
                    &task_ctx,
                    HttpErrorKind::Aborted,
                    "server closed before it finished draining",
                );
                if let Ok(error) = error {
                    let _ = reject.call::<_, ()>((error,));
                }
            }
        });
        Ok(promise)
    }
}

impl<'js> ServeOptions<'js> {
    pub fn serve(self, ctx: Ctx<'js>) -> Result<Server> {
        let (listener, addr) = self.listen.bind(&ctx)?;
        let url = format!("http://{addr}/");
        let counters = Rc::new(Counters::default());
        let dispatch = Rc::new(Dispatch::new(self.fetch, Rc::clone(&counters)));
        let (phase, receiver) = watch::channel(Phase::Serving);
        ctx.clone().spawn(
            AcceptLoop {
                ctx,
                listener,
                local: addr,
                dispatch,
                counters: Rc::clone(&counters),
                phase: receiver,
                done: phase.clone(),
            }
            .run(),
        );
        Ok(Server {
            addr,
            url,
            phase,
            counters,
        })
    }
}

pub fn socket_addr<'js>(ctx: &Ctx<'js>, addr: SocketAddr) -> Result<Object<'js>> {
    let value = Object::new(ctx.clone())?;
    value.set("hostname", addr.ip().to_string())?;
    value.set("port", addr.port())?;
    Ok(value)
}

struct AcceptLoop<'js> {
    ctx:      Ctx<'js>,
    listener: tokio::net::TcpListener,
    local:    SocketAddr,
    dispatch: Rc<Dispatch<'js>>,
    counters: Rc<Counters>,
    phase:    watch::Receiver<Phase>,
    done:     watch::Sender<Phase>,
}

impl AcceptLoop<'_> {
    async fn run(mut self) {
        let (alive, mut drained) = mpsc::channel(1);
        loop {
            let accepted = tokio::select! {
                _ = self.phase.changed() => break,
                accepted = self.listener.accept() => accepted,
            };
            let Ok((io, peer)) = accepted else {
                break;
            };
            let guard = self.counters.connection();
            let connection = Connection {
                ctx: self.ctx.clone(),
                io,
                peer,
                local: self.local,
                dispatch: Rc::clone(&self.dispatch),
                phase: self.phase.clone(),
                alive: alive.clone(),
            };
            self.ctx.clone().spawn(async move {
                let _guard = guard;
                connection.drive().await;
            });
        }
        drop(self.listener);
        drop(alive);
        loop {
            let phase = *self.phase.borrow();
            if phase.is_aborting() {
                break;
            }
            let Phase::Draining { deadline, .. } = phase else {
                break;
            };
            if let Some(deadline) = deadline {
                tokio::select! {
                    _ = drained.recv() => {
                        let _ = self.done.send(Phase::Done);
                        return;
                    }
                    _ = tokio::time::sleep_until(deadline) => {
                        let _ = self.done.send(Phase::Draining {
                            deadline: Some(deadline),
                            abort: true,
                        });
                    }
                    changed = self.phase.changed() => {
                        if changed.is_err() {
                            return;
                        }
                    }
                }
            } else {
                tokio::select! {
                    _ = drained.recv() => {
                        let _ = self.done.send(Phase::Done);
                        return;
                    }
                    changed = self.phase.changed() => {
                        if changed.is_err() {
                            return;
                        }
                    }
                }
            }
        }
        let _ = drained.recv().await;
        let _ = self.done.send(Phase::Done);
    }
}
