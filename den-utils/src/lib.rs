use std::{
    future::Future,
    io,
    pin::Pin,
    task::{Context, Poll},
};

use pin_project_lite::pin_project;
use tokio_util::sync::{CancellationToken, WaitForCancellationFuture};

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
