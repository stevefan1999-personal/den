use std::{convert::Infallible, net::SocketAddr, rc::Rc};

use den_stdlib_whatwg::fetch::Response;
use den_util::Probe as _;
use http::StatusCode;
use hyper::{Request as HyperRequest, body::Incoming};
use rquickjs::{Class, Ctx, Exception, Function, Object, Value, promise::MaybePromise};
use tokio::sync::{mpsc, watch};

use crate::{
    bridge::{Completion, DenBody, RequestBridge, WireResponse},
    serve::{Counters, Phase, socket_addr},
};

pub struct Dispatch<'js> {
    handler:  Function<'js>,
    counters: Rc<Counters>,
}

pub struct DispatchRequest<'js> {
    pub ctx:      Ctx<'js>,
    pub request:  HyperRequest<Incoming>,
    pub peer:     SocketAddr,
    pub local:    SocketAddr,
    pub base_url: String,
    pub phase:    watch::Receiver<Phase>,
    pub alive:    mpsc::Sender<Infallible>,
}

impl<'js> Dispatch<'js> {
    pub const fn new(handler: Function<'js>, counters: Rc<Counters>) -> Self {
        Self { handler, counters }
    }

    pub async fn handle(&self, request: DispatchRequest<'js>) -> WireResponse {
        let DispatchRequest {
            ctx,
            request,
            peer,
            local,
            base_url,
            phase,
            alive,
        } = request;
        let bridge = RequestBridge::from_wire(&ctx, request, &base_url).await;
        let (request, head, controller) = match bridge {
            Ok(RequestBridge::Dispatch {
                request,
                head,
                controller,
            }) => (request, head, controller),
            Ok(RequestBridge::BadRequest) => {
                return DenBody::error_response(StatusCode::BAD_REQUEST, "Bad Request\n");
            }
            Ok(RequestBridge::MethodNotAllowed) => {
                let mut response =
                    DenBody::error_response(StatusCode::METHOD_NOT_ALLOWED, "Method Not Allowed\n");
                response.headers_mut().insert(
                    http::header::ALLOW,
                    http::HeaderValue::from_static("GET, HEAD, POST, PUT, PATCH, DELETE, OPTIONS"),
                );
                return response;
            }
            Ok(RequestBridge::PayloadTooLarge) => {
                return DenBody::error_response(
                    StatusCode::PAYLOAD_TOO_LARGE,
                    "Payload Too Large\n",
                );
            }
            Err(error) => return Self::internal_error(&ctx, error),
        };

        let _pending = self.counters.request();
        let completion = Completion::watch(&ctx, controller, phase, alive);
        let info = match Self::connection_info(&ctx, peer, local) {
            Ok(info) => info,
            Err(error) => return Self::internal_error(&ctx, error),
        };
        let returned = match self.handler.call::<_, MaybePromise>((request, info)) {
            Ok(value) => value.into_future::<Value>().await,
            Err(error) => Err(error),
        };
        let value = match returned {
            Ok(value) => value,
            Err(error) => return Self::internal_error(&ctx, error),
        };
        let Some(response) = ctx.probe(|| Class::<Response>::from_value(&value).ok()) else {
            return Self::internal_error(
                &ctx,
                Exception::throw_type(&ctx, "den:http fetch handler must return a Response"),
            );
        };
        match DenBody::from_response(&ctx, &response, completion, head).await {
            Ok(response) => response,
            Err(error) => Self::internal_error(&ctx, error),
        }
    }

    fn connection_info(
        ctx: &Ctx<'js>, peer: SocketAddr, local: SocketAddr,
    ) -> rquickjs::Result<Object<'js>> {
        let info = Object::new(ctx.clone())?;
        info.set("remote", socket_addr(ctx, peer)?)?;
        info.set("local", socket_addr(ctx, local)?)?;
        Ok(info)
    }

    fn internal_error(ctx: &Ctx<'_>, error: rquickjs::Error) -> WireResponse {
        den_stdlib_core::exceptions::report_uncaught(ctx, Err(error));
        DenBody::error_response(StatusCode::INTERNAL_SERVER_ERROR, "Internal Server Error\n")
    }
}
