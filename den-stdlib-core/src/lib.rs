use once_cell::sync::Lazy;
use rquickjs::bind;

pub static WORLD_END: Lazy<tokio_util::sync::CancellationToken> =
    Lazy::new(|| tokio_util::sync::CancellationToken::new());

#[bind(object, public)]
#[quickjs(bare)]
pub mod cancellation_token {
    use std::ops::Deref;

    use delegate_attr::delegate;
    use derivative::Derivative;
    use derive_more::{Deref, DerefMut, From, Into};
    use tokio_util::sync::CancellationToken;

    #[quickjs(cloneable)]
    #[derive(Derivative, From, Into, Deref, DerefMut)]
    #[derivative(Clone, Debug)]
    pub struct CancellationTokenWrapper(CancellationToken);

    impl CancellationTokenWrapper {
        #[delegate(self.deref())]
        pub fn cancel(&self);
    }
}
