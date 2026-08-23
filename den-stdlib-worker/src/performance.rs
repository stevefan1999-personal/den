//! High-resolution clock backing `src/prelude/performance.js`.
//!
//! QuickJS-ng already installs a `performance` global (`JS_AddPerformance`)
//! whose `timeOrigin` is a monotonic reading, not Unix epoch. This clock is
//! captured when natives install and replaces that object so `timeOrigin` is
//! wall-clock and `now()` is milliseconds since this realm.

use std::time::{Instant, SystemTime, UNIX_EPOCH};

use rquickjs::{Ctx, Function, Object, Result};

/// Monotonic origin of one realm, plus the wall-clock reading of that moment.
pub struct PerformanceClock {
    origin: Instant,
    time_origin: f64,
}

impl PerformanceClock {
    fn capture() -> Self {
        let origin = Instant::now();
        let time_origin = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map(|elapsed| elapsed.as_secs_f64() * 1000.0)
            .unwrap_or(0.0);
        Self {
            origin,
            time_origin,
        }
    }

    fn now(&self) -> f64 {
        self.origin.elapsed().as_secs_f64() * 1000.0
    }

    /// `natives.now()` / `natives.timeOrigin` for the performance prelude.
    pub fn install<'js>(ctx: &Ctx<'js>, natives: &Object<'js>) -> Result<()> {
        let clock = Self::capture();
        natives.set("timeOrigin", clock.time_origin)?;
        natives.set("now", Function::new(ctx.clone(), move || clock.now())?)?;
        Ok(())
    }
}
