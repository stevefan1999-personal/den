use std::{convert::Infallible, future::Future, net::SocketAddr, rc::Rc, time::Duration};

use hyper::{Request as HyperRequest, body::Incoming, rt::Executor, service::service_fn};
use hyper_util::{
    rt::{TokioIo, TokioTimer},
    server::conn::auto,
};
use rquickjs::Ctx;
use tokio::{
    net::TcpStream,
    sync::{mpsc, oneshot, watch},
};

use crate::{
    bridge::{DenBody, WireResponse},
    dispatch::{Dispatch, DispatchRequest},
    serve::Phase,
};

#[derive(Clone)]
struct LocalExecutor<'js>(Ctx<'js>);

impl<'js, F> Executor<F> for LocalExecutor<'js>
where
    F: Future<Output = ()> + 'js,
{
    fn execute(&self, fut: F) { self.0.clone().spawn(fut); }
}

struct QueuedRequest {
    request: HyperRequest<Incoming>,
    respond: oneshot::Sender<WireResponse>,
}

pub struct Connection<'js> {
    pub ctx:      Ctx<'js>,
    pub io:       TcpStream,
    pub peer:     SocketAddr,
    pub local:    SocketAddr,
    pub dispatch: Rc<Dispatch<'js>>,
    pub phase:    watch::Receiver<Phase>,
    pub alive:    mpsc::Sender<Infallible>,
}

impl Connection<'_> {
    // ponytail: one realm is single-threaded; raise both caps together only
    // after handlers can make progress under more concurrent streams.
    const MAX_IN_FLIGHT_REQUESTS: usize = 32;

    pub async fn drive(self) {
        let Self {
            ctx,
            io,
            peer,
            local,
            dispatch,
            mut phase,
            alive,
        } = self;
        let local = io.local_addr().unwrap_or(local);
        let base_url = format!("http://{local}/");
        let (requests, mut queued) = mpsc::channel::<QueuedRequest>(Self::MAX_IN_FLIGHT_REQUESTS);
        let service = service_fn(move |request| {
            let (respond, response) = oneshot::channel();
            let requests = requests.clone();
            async move {
                let queued = requests
                    .send(QueuedRequest { request, respond })
                    .await
                    .is_ok();
                Ok::<_, Infallible>(if queued {
                    response.await.unwrap_or_else(|_| {
                        DenBody::error_response(
                            http::StatusCode::INTERNAL_SERVER_ERROR,
                            "Internal Server Error\n",
                        )
                    })
                } else {
                    DenBody::error_response(
                        http::StatusCode::SERVICE_UNAVAILABLE,
                        "Service Unavailable\n",
                    )
                })
            }
        });
        let mut builder = auto::Builder::new(LocalExecutor(ctx.clone()));
        builder
            .http1()
            .timer(TokioTimer::new())
            .header_read_timeout(Duration::from_secs(10));
        builder
            .http2()
            .max_concurrent_streams(Self::MAX_IN_FLIGHT_REQUESTS as u32);
        let connection = builder.serve_connection(TokioIo::new(io), service);
        let mut connection = std::pin::pin!(connection);
        let current = *phase.borrow();
        if current.is_aborting() {
            return;
        }
        if current.is_draining() {
            connection.as_mut().graceful_shutdown();
        }
        loop {
            tokio::select! {
                _ = connection.as_mut() => return,
                request = queued.recv() => {
                    let Some(QueuedRequest { request, mut respond }) = request else {
                        return;
                    };
                    let dispatch = Rc::clone(&dispatch);
                    let request = DispatchRequest {
                        ctx: ctx.clone(),
                        request,
                        peer,
                        local,
                        base_url: base_url.clone(),
                        phase: phase.clone(),
                        alive: alive.clone(),
                    };
                    ctx.clone().spawn(async move {
                        tokio::select! {
                            response = dispatch.handle(request) => {
                                let _ = respond.send(response);
                            }
                            _ = respond.closed() => {}
                        }
                    });
                }
                changed = phase.changed() => {
                    if changed.is_err() {
                        return;
                    }
                    let current = *phase.borrow();
                    if current.is_aborting() {
                        return;
                    }
                    if current.is_draining() {
                        connection.as_mut().graceful_shutdown();
                    }
                }
            }
        }
    }
}
