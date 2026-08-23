use std::{
    cell::{Cell, RefCell},
    collections::HashMap,
};

use rquickjs::{Ctx, JsLifetime, Result};
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
    fn install(ctx: &Ctx<'_>) -> Result<()> {
        if ctx.userdata::<Self>().is_some() {
            return Ok(());
        }
        ctx.store_userdata(Self::default())
            .map(|_| ())
            .map_err(|_| rquickjs::Exception::throw_internal(ctx, "timers are already installed"))
    }

    fn missing(ctx: &Ctx<'_>) -> rquickjs::Error {
        rquickjs::Exception::throw_internal(ctx, "timers are not installed")
    }

    /// Browsers start at 1 and never reuse an id that is still live.
    fn allocate(ctx: &Ctx<'_>, token: CancellationToken) -> Result<u32> {
        let timers = ctx.userdata::<Self>().ok_or_else(|| Self::missing(ctx))?;
        let mut handles = timers.handles.borrow_mut();
        loop {
            let id = timers.next.get().max(1);
            timers.next.set(id.wrapping_add(1));
            if handles.contains_key(&id) {
                continue;
            }
            handles.insert(id, token);
            return Ok(id);
        }
    }

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

/// Realm-wide stop signal. Not a JS class: `Engine::stop()` and Ctrl-C cancel
/// it so every `ctx.spawn` future (timers included) can complete and `idle()`
/// can return.
#[derive(Clone, JsLifetime)]
pub struct StopToken(pub CancellationToken);

impl StopToken {
    fn of(ctx: &Ctx<'_>) -> CancellationToken {
        ctx.userdata::<Self>()
            .map(|stop| stop.0.clone())
            .unwrap_or_default()
    }
}

#[rquickjs::module(
    rename = "camelCase",
    rename_vars = "camelCase",
    rename_types = "camelCase"
)]
pub mod timer {
    use std::time::Duration;

    use den_stdlib_core::report::report_exception;
    use rquickjs::{Coerced, Ctx, Error, FromJs, Result, Value, module::Exports, prelude::Opt};
    use tokio::time;
    use tokio_util::sync::CancellationToken;

    use super::{StopToken, Timers};

    /// What a zero delay means. Browsers clamp every timer to at least this
    /// (HTML §8.6 "timer initialisation steps" nests the clamp deeper, but the
    /// floor is the same), and tokio's `interval` refuses a zero period with a
    /// panic — which, on the main thread, takes the process down with it.
    const MINIMUM_DELAY: Duration = Duration::from_millis(1);

    /// A timer callback has nobody to propagate to: the caller returned long
    /// ago and the spawned task's result is dropped. Swallowing the exception
    /// here (`let _ = func.call(..)`) made a throwing `setTimeout` body a
    /// silent no-op, so it is reported like any other uncaught error instead —
    /// through the realm's sink, which in a worker is that worker's `error`
    /// event and then its parent's.
    fn report_uncaught(ctx: &Ctx<'_>, outcome: Result<()>) {
        match outcome {
            Ok(()) => {}
            Err(Error::Exception) => report_exception(ctx, &ctx.catch()),
            Err(error) => eprintln!("{error}"),
        }
    }

    fn delay_of(delay: Option<usize>) -> Duration {
        Duration::from_millis(delay.unwrap_or(0) as u64).max(MINIMUM_DELAY)
    }

    /// HTML: a function is called; anything else is coerced to a string and
    /// evaluated as a classic script (`setTimeout("x", 0)`).
    fn invoke<'js>(ctx: &Ctx<'js>, callback: &Value<'js>) {
        if let Some(func) = callback.as_function() {
            report_uncaught(ctx, func.call::<_, ()>(()));
            return;
        }
        match Coerced::<String>::from_js(ctx, callback.clone()) {
            Ok(Coerced(code)) => report_uncaught(ctx, ctx.eval::<(), _>(code)),
            Err(error) => report_uncaught(ctx, Err(error)),
        }
    }

    fn arm(ctx: &Ctx<'_>) -> Result<(u32, CancellationToken, CancellationToken)> {
        let token = CancellationToken::new();
        let id = Timers::allocate(ctx, token.clone())?;
        Ok((id, token, StopToken::of(ctx)))
    }

    #[rquickjs::function]
    #[qjs(rename = "setInterval")]
    pub fn set_interval<'js>(
        callback: Value<'js>, delay: Option<usize>, ctx: Ctx<'js>,
    ) -> Result<u32> {
        let mut interval = time::interval(delay_of(delay));
        interval.set_missed_tick_behavior(time::MissedTickBehavior::Delay);
        let (id, token, stop) = arm(&ctx)?;

        ctx.spawn({
            let ctx = ctx.clone();
            async move {
                let first = token
                    .run_until_cancelled(stop.run_until_cancelled(interval.tick()))
                    .await
                    .flatten();
                if first.is_some() {
                    while token
                        .run_until_cancelled(stop.run_until_cancelled(interval.tick()))
                        .await
                        .flatten()
                        .is_some()
                    {
                        invoke(&ctx, &callback);
                    }
                }
                Timers::forget(&ctx, id);
            }
        });

        Ok(id)
    }

    #[rquickjs::function]
    #[qjs(rename = "clearInterval")]
    pub fn clear_interval(id: Opt<u32>, ctx: Ctx<'_>) {
        if let Some(id) = id.0 {
            Timers::cancel(&ctx, id);
        }
    }

    #[rquickjs::function]
    #[qjs(rename = "setTimeout")]
    pub fn set_timeout<'js>(
        callback: Value<'js>, delay: Option<usize>, ctx: Ctx<'js>,
    ) -> Result<u32> {
        let duration = delay_of(delay);
        let (id, token, stop) = arm(&ctx)?;

        ctx.spawn({
            let ctx = ctx.clone();
            async move {
                let fired = token
                    .run_until_cancelled(stop.run_until_cancelled(time::sleep(duration)))
                    .await
                    .flatten()
                    .is_some();
                Timers::forget(&ctx, id);
                if fired {
                    invoke(&ctx, &callback);
                }
            }
        });
        Ok(id)
    }

    #[rquickjs::function]
    #[qjs(rename = "clearTimeout")]
    pub fn clear_timeout(id: Opt<u32>, ctx: Ctx<'_>) {
        if let Some(id) = id.0 {
            Timers::cancel(&ctx, id);
        }
    }

    #[qjs(evaluate)]
    pub fn evaluate<'js>(ctx: &Ctx<'js>, _: &Exports<'js>) -> Result<()> {
        Timers::install(ctx)?;
        let globals = ctx.globals();
        globals.set("setTimeout", js_set_timeout)?;
        globals.set("setInterval", js_set_interval)?;
        globals.set("clearTimeout", js_clear_timeout)?;
        globals.set("clearInterval", js_clear_interval)?;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use rquickjs::{
        AsyncContext, AsyncRuntime, CatchResultExt, FromJs, Module, Object, Promise,
        context::EvalOptions,
    };

    /// Evaluate `source` in a fresh realm with `den:timer` installed.
    ///
    /// The snippet may use top-level `await`; pending timers are drained with
    /// `idle()` only when the caller asks, because a still-live interval would
    /// otherwise hang the suite.
    async fn eval<T>(source: &str) -> Result<T, String>
    where
        T: for<'js> FromJs<'js> + Send + Sync + 'static,
    {
        let runtime = AsyncRuntime::new().expect("runtime");
        let context = AsyncContext::full(&runtime).await.expect("context");
        context
            .async_with(async |ctx| {
                let run = async {
                    let (_module, evaluated) =
                        Module::evaluate_def::<crate::js_timer, _>(ctx.clone(), "den:timer")?;
                    evaluated.into_future::<()>().await?;
                    let mut options = EvalOptions::default();
                    options.global = true;
                    options.promise = true;
                    options.strict = true;
                    ctx.eval_with_options::<Promise, _>(source, options)?
                        .into_future::<Object>()
                        .await?
                        .get::<_, T>("value")
                };
                run.await.catch(&ctx).map_err(|err| err.to_string())
            })
            .await
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn clear_timeout_is_a_function_and_set_timeout_returns_a_number() {
        let report: String = eval(
            r#"
              const id = setTimeout("x", 0);
              [typeof clearTimeout, typeof id].join(",")
            "#,
        )
        .await
        .expect("timer globals evaluate");
        assert_eq!(report, "function,number");
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn set_timeout_of_a_string_evaluates_it() {
        let ran: bool = eval(
            r#"
              setTimeout("globalThis.ran = true", 1);
              await new Promise((resolve) => setTimeout(resolve, 20));
              globalThis.ran === true
            "#,
        )
        .await
        .expect("string timer evaluates");
        assert!(ran);
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn clear_timeout_of_a_numeric_id_cancels_the_callback() {
        let outcome: String = eval(
            r#"
              let fired = false;
              const pending = setTimeout(() => { fired = true; }, 50);
              clearTimeout(pending);
              await new Promise((resolve) => setTimeout(resolve, 1));
              fired ? "fired anyway" : "cancelled"
            "#,
        )
        .await
        .expect("clearTimeout evaluates");
        assert_eq!(outcome, "cancelled");
    }
}
