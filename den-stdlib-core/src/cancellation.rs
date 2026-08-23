use std::ops::Deref;

use delegate_attr::delegate;
use derivative::Derivative;
use derive_more::{Deref, DerefMut, From, Into};
use rquickjs::{JsLifetime, class::Trace};
pub use tokio_util::sync::CancellationToken;

#[derive(Trace, JsLifetime, Derivative, Default, From, Into, Deref, DerefMut)]
#[derivative(Clone, Debug)]
#[rquickjs::class(rename = "CancellationToken")]
pub struct CancellationTokenWrapper {
    #[qjs(skip_trace)]
    pub token: CancellationToken,
}

#[rquickjs::methods]
impl CancellationTokenWrapper {
    #[qjs(constructor)]
    pub fn new() -> Self {
        Self {
            token: CancellationToken::new(),
        }
    }

    #[delegate(self.deref())]
    pub fn cancel(&self) {}
}
