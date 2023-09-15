use derivative::Derivative;
use derive_more::{Deref, DerefMut, From, Into};
use once_cell::sync::Lazy;
use rquickjs::class::Trace;
use tokio_util::sync::CancellationToken;

pub static WORLD_END: Lazy<tokio_util::sync::CancellationToken> =
    Lazy::new(|| tokio_util::sync::CancellationToken::new());

#[derive(Trace, Derivative, From, Into, Deref, DerefMut)]
#[derivative(Clone, Debug)]
#[rquickjs::class(rename = "CancellationToken")]
pub struct CancellationTokenWrapper {
    #[qjs(skip_trace)]
    token: CancellationToken,
}

#[rquickjs::methods]
impl CancellationTokenWrapper {
    #[delegate(self.deref())]
    pub fn cancel(&self);
}

#[rquickjs::module]
pub mod core {
    pub use super::CancellationTokenWrapper;
}
