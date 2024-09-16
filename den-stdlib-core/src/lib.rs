pub mod cancellation;

#[rquickjs::module]
pub mod core {
    pub use crate::cancellation::CancellationTokenWrapper;
}
