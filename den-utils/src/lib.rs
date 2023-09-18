use std::{
    future::Future,
    io,
    pin::Pin,
    task::{Context, Poll},
};

use pin_project_lite::pin_project;
use tokio_util::sync::{CancellationToken, WaitForCancellationFuture};
#[cfg(feature = "transpile")]
use {
    color_eyre::eyre,
    color_eyre::eyre::eyre,
    swc_core::ecma::parser::{EsConfig, Syntax, TsConfig},
};

pin_project! {
    #[must_use = "futures do nothing unless polled"]
    pub struct WithCancellationFuture<'a, T: Future>{
        #[pin]
        future: T,
        #[pin]
        cancellation: WaitForCancellationFuture<'a>,
    }
}

impl<T: Future> Future for WithCancellationFuture<'_, T> {
    type Output = Result<T::Output, io::Error>;

    fn poll(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Self::Output> {
        let mut this = self.project();

        if let Poll::Ready(()) = this.cancellation.as_mut().poll(cx) {
            Poll::Ready(Err(io::Error::from(io::ErrorKind::TimedOut)))
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
    fn with_cancellation(
        self,
        token: &CancellationToken,
    ) -> WithCancellationFuture<'_, Self::Future>;
}

impl<T: Future> FutureExt for T {
    type Future = T;

    fn with_cancellation(self, token: &CancellationToken) -> WithCancellationFuture<'_, T> {
        WithCancellationFuture {
            future:       self,
            cancellation: token.cancelled(),
        }
    }
}

#[cfg(feature = "transpile")]
pub fn infer_transpile_syntax_by_extension(extension: &str) -> eyre::Result<Syntax> {
    trie_match::trie_match! {
        match extension {
            "js" | "mjs" => { Some(Syntax::Es(Default::default())) }
            "jsx" | "mjsx" => {
                if cfg!(feature = "react") {
                    Some(Syntax::Es(EsConfig { jsx: true, ..Default::default() }))
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
                    Some(Syntax::Typescript(TsConfig { tsx: true, ..Default::default() }))
                } else {
                    None
                }
            }
            _ => { None }
        }
    }
    .ok_or(eyre!("invalid extension"))
}

#[cfg(feature = "transpile")]
pub fn get_best_transpiling() -> &'static str {
    match (cfg!(feature = "typescript"), cfg!(feature = "react")) {
        (false, false) => "js",
        (false, true) => "jsx",
        (true, false) => "ts",
        (true, true) => "tsx",
    }
}
