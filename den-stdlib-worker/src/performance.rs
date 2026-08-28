//! High Resolution Time `performance.now` / `timeOrigin`.
//!
//! QuickJS-ng already installs a `performance` global (`JS_AddPerformance`)
//! whose `timeOrigin` is a monotonic reading, not Unix epoch. This class
//! replaces that object so `timeOrigin` is wall-clock and `now()` is
//! milliseconds since this realm.

use std::time::{Instant, SystemTime, UNIX_EPOCH};

use rquickjs::{Class, Ctx, JsLifetime, Result, atom::PredefinedAtom, class::Trace};

/// Monotonic origin of one realm, plus the wall-clock reading of that moment.
#[derive(Trace)]
#[rquickjs::class]
pub struct Performance {
    #[qjs(skip_trace)]
    origin:      Instant,
    #[qjs(get, rename = "timeOrigin")]
    time_origin: f64,
}

unsafe impl<'js> JsLifetime<'js> for Performance {
    type Changed<'to> = Performance;
}

impl Performance {
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

    /// The realm's `performance` object.
    pub fn instance<'js>(ctx: &Ctx<'js>) -> Result<Class<'js, Self>> {
        Class::instance(ctx.clone(), Self::capture())
    }
}

#[rquickjs::methods(rename_all = "camelCase")]
impl Performance {
    pub fn now(&self) -> f64 { self.origin.elapsed().as_secs_f64() * 1000.0 }

    #[qjs(prop, rename = PredefinedAtom::SymbolToStringTag, configurable)]
    pub fn to_string_tag() -> &'static str { "Performance" }
}
