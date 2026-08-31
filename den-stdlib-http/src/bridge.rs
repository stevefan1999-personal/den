use std::{
    convert::Infallible,
    pin::Pin,
    task::{Context, Poll},
};

use bytes::Bytes;
use den_stdlib_whatwg::fetch::{BufferedResponse, Request, Response, ServerRequest};
use http::{HeaderName, HeaderValue, Method, StatusCode, Version};
use http_body_util::{BodyExt as _, LengthLimitError, Limited};
use hyper::{
    Request as HyperRequest, Response as HyperResponse,
    body::{Body, Frame, Incoming, SizeHint},
};
use rquickjs::{Class, Ctx, Function, Object, Result, function::This};
use tokio::sync::{mpsc, oneshot, watch};

use crate::serve::Phase;

pub type WireResponse = HyperResponse<DenBody>;

pub struct Completion(Option<oneshot::Sender<()>>);

impl Completion {
    pub fn watch<'js>(
        ctx: &Ctx<'js>, controller: Object<'js>, mut phase: watch::Receiver<Phase>,
        alive: mpsc::Sender<Infallible>,
    ) -> Self {
        let (sender, receiver) = oneshot::channel();
        let watcher_ctx = ctx.clone();
        ctx.clone().spawn(async move {
            let _alive = alive;
            let mut receiver = receiver;
            let aborted = loop {
                if phase.borrow().is_aborting() {
                    break true;
                }
                tokio::select! {
                    outcome = &mut receiver => break outcome.is_ok(),
                    changed = phase.changed() => {
                        if changed.is_err() {
                            break true;
                        }
                    }
                }
            };
            if aborted {
                let abort = controller
                    .get::<_, Function>("abort")
                    .and_then(|abort| abort.call::<_, ()>((This(controller),)));
                den_stdlib_core::exceptions::report_uncaught(&watcher_ctx, abort);
            }
        });
        Self(Some(sender))
    }

    fn finish(&mut self) { drop(self.0.take()); }
}

impl Drop for Completion {
    fn drop(&mut self) {
        if let Some(sender) = self.0.take() {
            let _ = sender.send(());
        }
    }
}

pub struct DenBody {
    bytes:      Option<Bytes>,
    completion: Option<Completion>,
}

impl DenBody {
    // ponytail: phase 1 is intentionally buffered. Replace this cap with
    // backpressured bodies when streaming lands; never remove the boundary.
    pub const MAX_BUFFERED_BODY_BYTES: usize = 16 * 1024 * 1024;

    pub const fn plain(text: &'static str) -> Self {
        Self {
            bytes:      Some(Bytes::from_static(text.as_bytes())),
            completion: None,
        }
    }

    fn from_buffered(
        ctx: &Ctx<'_>, response: BufferedResponse, completion: Completion, head: bool,
    ) -> Result<WireResponse> {
        let bytes = Bytes::from(response.body);
        let representation_length = bytes.len();
        let completion = if head {
            let mut completion = completion;
            completion.finish();
            None
        } else {
            Some(completion)
        };
        let body = Self {
            bytes: (!head && !bytes.is_empty()).then_some(bytes),
            completion,
        };

        let mut outgoing = HyperResponse::new(body);
        *outgoing.status_mut() = StatusCode::from_u16(response.status)
            .map_err(|error| rquickjs::Exception::throw_type(ctx, &error.to_string()))?;
        for (name, value) in response.headers {
            let name = HeaderName::try_from(name)
                .map_err(|error| rquickjs::Exception::throw_type(ctx, &error.to_string()))?;
            if matches!(
                name,
                http::header::CONTENT_LENGTH | http::header::TRANSFER_ENCODING
            ) {
                continue;
            }
            let value = HeaderValue::try_from(value)
                .map_err(|error| rquickjs::Exception::throw_type(ctx, &error.to_string()))?;
            outgoing.headers_mut().append(name, value);
        }
        if head {
            let length = HeaderValue::try_from(representation_length.to_string())
                .map_err(|error| rquickjs::Exception::throw_type(ctx, &error.to_string()))?;
            outgoing
                .headers_mut()
                .insert(http::header::CONTENT_LENGTH, length);
        }
        Ok(outgoing)
    }

    pub async fn from_response<'js>(
        ctx: &Ctx<'js>, response: &Class<'js, Response<'js>>, completion: Completion, head: bool,
    ) -> Result<WireResponse> {
        let response = Response::into_server(response, ctx, Self::MAX_BUFFERED_BODY_BYTES).await?;
        Self::from_buffered(ctx, response, completion, head)
    }

    pub fn error_response(status: StatusCode, body: &'static str) -> WireResponse {
        let mut response = HyperResponse::new(Self::plain(body));
        *response.status_mut() = status;
        response.headers_mut().insert(
            http::header::CONTENT_TYPE,
            HeaderValue::from_static("text/plain; charset=utf-8"),
        );
        response
    }
}

impl Body for DenBody {
    type Data = Bytes;
    type Error = Infallible;

    fn poll_frame(
        mut self: Pin<&mut Self>, _ctx: &mut Context<'_>,
    ) -> Poll<Option<std::result::Result<Frame<Self::Data>, Self::Error>>> {
        let frame = self.bytes.take().map(Frame::data).map(Ok);
        if let Some(mut completion) = self.completion.take() {
            completion.finish();
        }
        Poll::Ready(frame)
    }

    fn is_end_stream(&self) -> bool { self.bytes.is_none() && self.completion.is_none() }

    fn size_hint(&self) -> SizeHint {
        SizeHint::with_exact(self.bytes.as_ref().map_or(0, |bytes| bytes.len() as u64))
    }
}

pub enum RequestBridge<'js> {
    Dispatch {
        request:    Class<'js, Request<'js>>,
        head:       bool,
        controller: Object<'js>,
    },
    BadRequest,
    MethodNotAllowed,
    PayloadTooLarge,
}

impl<'js> RequestBridge<'js> {
    pub async fn from_wire(
        ctx: &Ctx<'js>, request: HyperRequest<Incoming>, base_url: &str,
    ) -> Result<Self> {
        let method = request.method().clone();
        let version = request.version();
        if matches!(method.as_str(), "CONNECT" | "TRACE" | "TRACK") {
            return Ok(Self::MethodNotAllowed);
        }
        let uri = request.uri();
        let Some(target) = uri.path_and_query().map(http::uri::PathAndQuery::as_str) else {
            return Ok(Self::BadRequest);
        };
        if !target.starts_with('/')
            || (version != Version::HTTP_2 && (uri.scheme().is_some() || uri.authority().is_some()))
            || (version == Version::HTTP_2 && uri.scheme_str() != Some("http"))
        {
            return Ok(Self::BadRequest);
        }
        let url = format!("{}{target}", base_url.trim_end_matches('/'));
        let mut headers: Vec<_> = request
            .headers()
            .iter()
            .map(|(name, value)| {
                let value = value.to_str().map_or_else(
                    |_error| {
                        value
                            .as_bytes()
                            .iter()
                            .map(|byte| char::from(*byte))
                            .collect()
                    },
                    ToOwned::to_owned,
                );
                (name.as_str().to_string(), value)
            })
            .collect();
        if version == Version::HTTP_2
            && !headers.iter().any(|(name, _)| name == "host")
            && let Some(authority) = uri.authority()
        {
            headers.push(("host".to_string(), authority.to_string()));
        }
        if request
            .body()
            .size_hint()
            .upper()
            .is_some_and(|bytes| bytes > DenBody::MAX_BUFFERED_BODY_BYTES as u64)
        {
            return Ok(Self::PayloadTooLarge);
        }
        let body = match Limited::new(request.into_body(), DenBody::MAX_BUFFERED_BODY_BYTES)
            .collect()
            .await
        {
            Ok(body) => body.to_bytes().to_vec(),
            Err(error) if error.downcast_ref::<LengthLimitError>().is_some() => {
                return Ok(Self::PayloadTooLarge);
            }
            Err(_) => return Ok(Self::BadRequest),
        };
        if matches!(method, Method::GET | Method::HEAD) && !body.is_empty() {
            return Ok(Self::BadRequest);
        }
        let head = method == Method::HEAD;
        let controller: Object = den_util::construct(ctx, "AbortController", ())?;
        let signal = controller.get("signal")?;
        let request = Request::from_server(ctx, ServerRequest {
            url,
            method: method.as_str().to_string(),
            headers,
            body,
            signal,
        })?;
        Ok(Self::Dispatch {
            request: Class::instance(ctx.clone(), request)?,
            head,
            controller,
        })
    }
}
