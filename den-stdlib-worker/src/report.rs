//! Re-exports of the reporting API, which lives in `den-stdlib-core` so that
//! crates with no reason to depend on the worker crate (the timer stdlib, say)
//! can reach it too.
//!
//! `sink_hook` is here because the sink *is* this crate's private natives bag
//! (`events::install` parks it), so an embedder reaching for one of the hooks
//! the preludes publish — `dispatchTrusted` — is by definition a `den:worker`
//! user and should not have to name `den-stdlib-core` to say so.

pub use den_stdlib_core::exceptions::{report_exception, report_uncaught, sink_hook};
