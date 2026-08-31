#[cfg(feature = "package-store")]
use std::sync::Arc;

use den_capabilities::Policy;
#[cfg(feature = "package-store")]
use den_package_store::PackageModuleSnapshot;
use rquickjs::loader::Bundle;

use crate::engine::Engine;

/// QuickJS' native defaults, made explicit so worker engines cannot silently
/// reset them while cloning the rest of the host configuration.
pub const DEFAULT_MAX_STACK_SIZE: usize = 1024 * 1024;
pub const DEFAULT_GC_THRESHOLD: usize = 256 * 1024;

const EMPTY_BUNDLE: Bundle = rquickjs::loader::bundle::Bundle(&rquickjs::phf::Map::new());

#[derive(Clone, Debug)]
pub(crate) struct EngineSettings {
    pub(crate) max_stack_size:  usize,
    pub(crate) gc_threshold:    usize,
    pub(crate) heap_limit:      Option<usize>,
    pub(crate) policy:          Policy,
    pub(crate) argv:            Vec<String>,
    #[cfg(feature = "package-store")]
    pub(crate) package_modules: Option<Arc<PackageModuleSnapshot>>,
}

impl Default for EngineSettings {
    fn default() -> Self {
        Self {
            max_stack_size:                                    DEFAULT_MAX_STACK_SIZE,
            gc_threshold:                                      DEFAULT_GC_THRESHOLD,
            heap_limit:                                        None,
            policy:                                            Policy::default(),
            argv:                                              std::env::args_os()
                .map(|arg| arg.to_string_lossy().into_owned())
                .collect(),
            #[cfg(feature = "package-store")]
            package_modules:                                   None,
        }
    }
}

/// Configuration for one den realm. Dedicated workers receive a clone of the
/// same settings, including deny-by-default authority and resource limits.
#[derive(Clone, Debug)]
#[expect(
    clippy::module_name_repetitions,
    reason = "EngineBuilder is the public builder paired with Engine"
)]
pub struct EngineBuilder {
    bundle:   Bundle,
    settings: EngineSettings,
}

impl EngineBuilder {
    #[must_use]
    pub fn new() -> Self { Self::default() }

    #[must_use]
    pub const fn bundle(mut self, bundle: Bundle) -> Self {
        self.bundle = bundle;
        self
    }

    #[must_use]
    pub const fn max_stack_size(mut self, bytes: usize) -> Self {
        self.settings.max_stack_size = bytes;
        self
    }

    #[must_use]
    pub const fn gc_threshold(mut self, bytes: usize) -> Self {
        self.settings.gc_threshold = bytes;
        self
    }

    #[must_use]
    pub const fn heap_limit(mut self, bytes: usize) -> Self {
        self.settings.heap_limit = Some(bytes);
        self
    }

    #[must_use]
    pub fn policy(mut self, policy: Policy) -> Self {
        self.settings.policy = policy;
        self
    }

    #[must_use]
    pub fn argv(mut self, argv: Vec<String>) -> Self {
        self.settings.argv = argv;
        self
    }

    #[cfg(feature = "package-store")]
    #[must_use]
    pub fn package_modules(mut self, snapshot: Arc<PackageModuleSnapshot>) -> Self {
        self.settings.package_modules = Some(snapshot);
        self
    }

    pub async fn build(self) -> Engine { Engine::build(self.bundle, self.settings).await }
}

impl Default for EngineBuilder {
    fn default() -> Self {
        Self {
            bundle:   EMPTY_BUNDLE,
            settings: EngineSettings::default(),
        }
    }
}

#[cfg(test)]
#[path = "../tests/unit/builder.rs"]
mod tests;
