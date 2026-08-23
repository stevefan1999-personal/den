//! What a worker thread needs from its embedder, and nothing more.
//!
//! den-core owns engine construction (loaders, transpiler, the whole stdlib);
//! this crate owns threads and messages. The seam between them is one trait
//! with one method, stored in the context userdata so that a worker context —
//! which gets the very same slot — can spawn workers of its own.

use core::fmt::{self, Display};
use std::sync::Arc;

use rquickjs::{AsyncContext, AsyncRuntime, JsLifetime};
use tokio_util::sync::CancellationToken;

/// The realm's API base URL: what a relative worker script URL resolves
/// against, e.g. `"file:///home/me/project/"`.
///
/// Directory-shaped (trailing slash) because [`url::Url::join`] drops the last
/// path segment otherwise. Stored as context userdata by whoever builds the
/// context; a worker inherits its own base from the script it loaded.
#[derive(Clone, Debug, JsLifetime)]
pub struct BaseUrl(pub String);

/// A runtime and its context, built on — and owned by — one worker thread.
///
/// The two are kept together because dropping the runtime before the context
/// is a use-after-free, and because the thread body needs both: the context to
/// install the global scope and run the script, the runtime to drive the event
/// loop with [`AsyncRuntime::idle`].
pub struct WorkerEngine {
    pub runtime: AsyncRuntime,
    pub context: AsyncContext,
}

/// Why a worker engine could not be built. A string, because the embedder's
/// error type is den-core's business and this crate must not depend on it.
#[derive(Clone, Debug)]
pub struct WorkerHostError(pub String);

impl Display for WorkerHostError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        Display::fmt(&self.0, formatter)
    }
}

impl std::error::Error for WorkerHostError {}

/// The embedder's engine factory.
///
/// Lifetime: one `Arc<dyn WorkerHost>` per process (singleton), cloned into the
/// userdata of every context that may run `new Worker` — worker contexts
/// included, which is what makes nesting free.
pub trait WorkerHost: Send + Sync + 'static {
    /// Build a complete engine: runtime, context, loaders, every stdlib module
    /// including `den:worker`, and an interrupt handler that observes `stop`.
    ///
    /// Called **on the worker's own OS thread**, inside that thread's tokio
    /// runtime context, before any script runs. An implementor whose engine
    /// construction is `async` blocks on it there; nothing else is running on
    /// that thread yet.
    fn build_engine(
        &self, stop: CancellationToken, base: BaseUrl,
    ) -> Result<WorkerEngine, WorkerHostError>;
}

/// Userdata slot holding the host. `new Worker` reads it; contexts built
/// without one simply cannot spawn workers.
#[derive(Clone, JsLifetime)]
pub struct HostHandle(pub Arc<dyn WorkerHost>);
