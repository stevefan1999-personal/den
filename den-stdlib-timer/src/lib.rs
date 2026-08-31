use std::{
    cell::{Cell, RefCell},
    collections::HashMap,
};

use rquickjs::{Ctx, JsLifetime};
use tokio_util::sync::CancellationToken;

/// Per-realm timer handles. Scripts see a numeric id, never this map.
#[derive(JsLifetime)]
struct Timers {
    next:    Cell<u32>,
    handles: RefCell<HashMap<u32, CancellationToken>>,
}

impl Default for Timers {
    fn default() -> Self {
        Self {
            next:    Cell::new(1),
            handles: RefCell::default(),
        }
    }
}

impl Timers {
    fn cancel(ctx: &Ctx<'_>, id: u32) {
        if let Some(timers) = ctx.userdata::<Self>()
            && let Some(token) = timers.handles.borrow_mut().remove(&id)
        {
            token.cancel();
        }
    }

    fn forget(ctx: &Ctx<'_>, id: u32) {
        if let Some(timers) = ctx.userdata::<Self>() {
            timers.handles.borrow_mut().remove(&id);
        }
    }
}

#[rquickjs::module(
    rename = "camelCase",
    rename_vars = "camelCase",
    rename_types = "camelCase"
)]
pub mod timer {
    use std::time::Duration;

    use den_stdlib_core::exceptions::report_uncaught;
    use rquickjs::{Ctx, Function, Result, Value, function::Rest, module::Exports, prelude::Opt};
    use tokio::time;
    use tokio_util::sync::CancellationToken;

    use super::Timers;

    /// What a zero delay means. Browsers clamp every timer to at least this
    /// (HTML §8.6 "timer initialisation steps" nests the clamp deeper, but the
    /// floor is the same), and tokio's `interval` refuses a zero period with a
    /// panic — which, on the main thread, takes the process down with it.
    const MINIMUM_DELAY: Duration = Duration::from_millis(1);

    fn delay_of(delay: Option<usize>) -> Duration {
        Duration::from_millis(delay.unwrap_or(0) as u64).max(MINIMUM_DELAY)
    }

    fn invoke<'js>(ctx: &Ctx<'js>, callback: &Function<'js>, arguments: &[Value<'js>]) {
        report_uncaught(ctx, callback.call::<_, ()>((Rest(arguments.to_vec()),)));
    }

    fn arm(ctx: &Ctx<'_>) -> Result<(u32, CancellationToken)> {
        let timers = ctx
            .userdata::<Timers>()
            .ok_or_else(|| rquickjs::Exception::throw_internal(ctx, "timers are not installed"))?;
        // Browsers start at 1 and never reuse an id that is still live.
        let token = CancellationToken::new();
        let mut handles = timers.handles.borrow_mut();
        let id = loop {
            let id = timers.next.get().max(1);
            timers.next.set(id.wrapping_add(1));
            if !handles.contains_key(&id) {
                break id;
            }
        };
        handles.insert(id, token.clone());
        Ok((id, token))
    }

    // The macro injects `Ctx` by value, and the body only borrows it; a
    // reference parameter is not an option at this boundary.
    #[rquickjs::function]
    #[qjs(rename = "setInterval")]
    pub fn set_interval<'js>(
        callback: Function<'js>, delay: Option<usize>, Rest(arguments): Rest<Value<'js>>,
        ctx: Ctx<'js>,
    ) -> Result<u32> {
        let mut interval = time::interval(delay_of(delay));
        interval.set_missed_tick_behavior(time::MissedTickBehavior::Delay);
        let (id, token) = arm(&ctx)?;

        ctx.spawn({
            let ctx = ctx.clone();
            async move {
                // The first tick is immediate, and firing on it would run the
                // callback before the delay elapsed.
                if token.run_until_cancelled(interval.tick()).await.is_some() {
                    while token.run_until_cancelled(interval.tick()).await.is_some() {
                        invoke(&ctx, &callback, &arguments);
                    }
                }
                Timers::forget(&ctx, id);
            }
        });

        Ok(id)
    }

    #[rquickjs::function]
    #[qjs(rename = "clearInterval")]
    pub fn clear_interval(Opt(id): Opt<u32>, ctx: Ctx<'_>) {
        if let Some(id) = id {
            Timers::cancel(&ctx, id);
        }
    }

    #[rquickjs::function]
    #[qjs(rename = "setTimeout")]
    pub fn set_timeout<'js>(
        callback: Function<'js>, delay: Option<usize>, Rest(arguments): Rest<Value<'js>>,
        ctx: Ctx<'js>,
    ) -> Result<u32> {
        let duration = delay_of(delay);
        let (id, token) = arm(&ctx)?;

        ctx.spawn({
            let ctx = ctx.clone();
            async move {
                let fired = token
                    .run_until_cancelled(time::sleep(duration))
                    .await
                    .is_some();
                Timers::forget(&ctx, id);
                if fired {
                    invoke(&ctx, &callback, &arguments);
                }
            }
        });
        Ok(id)
    }

    #[rquickjs::function]
    #[qjs(rename = "clearTimeout")]
    pub fn clear_timeout(Opt(id): Opt<u32>, ctx: Ctx<'_>) {
        if let Some(id) = id {
            Timers::cancel(&ctx, id);
        }
    }

    #[rquickjs::function]
    #[qjs(rename = "setImmediate")]
    pub fn set_immediate<'js>(
        callback: Function<'js>, Rest(arguments): Rest<Value<'js>>, ctx: Ctx<'js>,
    ) -> Result<u32> {
        let (id, token) = arm(&ctx)?;
        ctx.spawn({
            let ctx = ctx.clone();
            async move {
                let fired = token
                    .run_until_cancelled(tokio::task::yield_now())
                    .await
                    .is_some();
                Timers::forget(&ctx, id);
                if fired {
                    invoke(&ctx, &callback, &arguments);
                }
            }
        });
        Ok(id)
    }

    #[rquickjs::function]
    #[qjs(rename = "clearImmediate")]
    pub fn clear_immediate(Opt(id): Opt<u32>, ctx: Ctx<'_>) {
        if let Some(id) = id {
            Timers::cancel(&ctx, id);
        }
    }

    #[qjs(evaluate)]
    pub fn evaluate<'js>(ctx: &Ctx<'js>, _: &Exports<'js>) -> Result<()> {
        if ctx.userdata::<Timers>().is_none() {
            ctx.store_userdata(Timers::default())
                .map(|_| ())
                .map_err(|_error| {
                    rquickjs::Exception::throw_internal(ctx, "timers are already installed")
                })?;
        }
        let globals = ctx.globals();
        globals.set("setTimeout", js_set_timeout)?;
        globals.set("setInterval", js_set_interval)?;
        globals.set("clearTimeout", js_clear_timeout)?;
        globals.set("clearInterval", js_clear_interval)?;
        globals.set("setImmediate", js_set_immediate)?;
        globals.set("clearImmediate", js_clear_immediate)?;
        Ok(())
    }
}
