use derivative::Derivative;
use derive_more::{Deref, DerefMut, From, Into};
use rquickjs::class::Trace;
use tokio_util::sync::CancellationToken;

#[derive(Trace, Derivative, From, Into, Deref, DerefMut)]
#[derivative(Clone, Debug)]
#[rquickjs::class(rename = "CancellationToken")]
pub struct CancellationTokenWrapper {
    #[qjs(skip_trace)]
    pub token: CancellationToken,
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

pub trait WorldsEndExt {
    fn worlds_end(&self) -> CancellationToken;
}

impl WorldsEndExt for rquickjs::Ctx<'_> {
    fn worlds_end(&self) -> CancellationToken {
        self.globals()
            .get::<_, CancellationTokenWrapper>("WORLD_END")
            .map(|x| x.token.child_token())
            .unwrap_or_else(|_| CancellationToken::new())
    }
}
