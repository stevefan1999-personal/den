use delegate_attr::delegate;
use derivative::Derivative;
use derive_more::{Deref, DerefMut, From, Into};
use rquickjs::{class::Trace, JsLifetime};
pub use tokio_util::sync::CancellationToken;

#[derive(Trace, JsLifetime, Derivative, From, Into, Deref, DerefMut)]
#[derivative(Clone, Debug)]
#[rquickjs::class(rename = "CancellationToken")]
pub struct CancellationTokenWrapper {
    #[qjs(skip_trace)]
    pub token: CancellationToken,
}

#[rquickjs::methods]
impl CancellationTokenWrapper {
    #[qjs(constructor)]
    pub fn new() {}

    #[delegate(self.deref())]
    pub fn cancel(&self) {}
}
