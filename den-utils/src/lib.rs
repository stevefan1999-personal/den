use std::{
    future::Future,
    io,
    pin::Pin,
    task::{Context, Poll},
};

use pin_project_lite::pin_project;
use tokio_util::sync::{CancellationToken, WaitForCancellationFutureOwned};
#[cfg(feature = "transpile")]
use {
    derive_more::{Debug, Display, Error, From},
    swc_core::ecma::parser::{EsSyntax, Syntax, TsSyntax},
};

pin_project! {
    #[must_use = "futures do nothing unless polled"]
    pub struct WithCancellationFuture<T: Future>{
        #[pin]
        future: T,
        #[pin]
        cancellation: WaitForCancellationFutureOwned,
    }
}

impl<T: Future> Future for WithCancellationFuture<T> {
    type Output = Result<T::Output, io::Error>;

    fn poll(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Self::Output> {
        let mut this = self.project();

        if let Poll::Ready(()) = this.cancellation.as_mut().poll(cx) {
            Poll::Ready(Err(io::Error::from(io::ErrorKind::Interrupted)))
        } else {
            match this.future.as_mut().poll(cx) {
                Poll::Ready(res) => Poll::Ready(Ok(res)),
                Poll::Pending => Poll::Pending,
            }
        }
    }
}

pub trait FutureExt {
    type Future: Future;
    fn with_cancellation(self, token: &CancellationToken) -> WithCancellationFuture<Self::Future>;
}

impl<T: Future> FutureExt for T {
    type Future = T;

    fn with_cancellation(self, token: &CancellationToken) -> WithCancellationFuture<T> {
        WithCancellationFuture {
            future:       self,
            cancellation: token.clone().cancelled_owned(),
        }
    }
}

#[cfg(feature = "transpile")]
pub fn infer_transpile_syntax_by_extension(
    extension: &str,
) -> Result<Syntax, InferTranspileSyntaxError> {
    trie_match::trie_match! {
        match extension {
            "js" | "mjs" => { Some(Syntax::Es(Default::default())) }
            "jsx" | "mjsx" => {
                if cfg!(feature = "react") {
                    Some(Syntax::Es(EsSyntax { jsx: true, ..Default::default() }))
                } else {
                    None
                }
            }
            "ts" => {
                if cfg!(feature = "typescript") {
                    Some(Syntax::Typescript(Default::default()))
                } else {
                    None
                }
            }
            "tsx" => {
                if cfg!(all(feature = "typescript", feature = "react")) {
                    Some(Syntax::Typescript(TsSyntax { tsx: true, ..Default::default() }))
                } else {
                    None
                }
            }
            _ => { None }
        }
    }
    .ok_or(InferTranspileSyntaxError::InvalidExtension)
}

#[cfg(feature = "transpile")]
#[derive(Display, From, Error, Debug)]
pub enum InferTranspileSyntaxError {
    InvalidExtension,
}

#[cfg(feature = "transpile")]
pub const fn get_best_transpiling() -> &'static str {
    match (cfg!(feature = "typescript"), cfg!(feature = "react")) {
        (false, false) => "js",
        (false, true) => "jsx",
        (true, false) => "ts",
        (true, true) => "tsx",
    }
}
