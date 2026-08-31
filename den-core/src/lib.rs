/// The FFI capability value, re-exported so that a host — den's own CLI — can
/// mint one and hand it to a realm without naming `den-stdlib-ffi` itself.
#[cfg(feature = "stdlib-ffi")]
pub use den_stdlib_ffi::FfiGrant;

pub mod builder;
pub mod engine;
pub mod loader;
#[cfg(feature = "package-store")] pub mod package;
pub mod resolver;

pub use builder::EngineBuilder;
