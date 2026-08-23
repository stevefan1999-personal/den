#[rquickjs::module(
    rename = "camelCase",
    rename_vars = "camelCase",
    rename_types = "camelCase"
)]
pub mod timer {
    use std::time::Duration;

    use den_stdlib_core::{
        cancellation::{CancellationToken, CancellationTokenWrapper},
        report::report_exception,
    };
    use rquickjs::{
        Ctx, Error, Function, Result,
        module::{Declarations, Exports},
    };
    use tokio::time;

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

    #[rquickjs::function(rename = "setInterval")]
    pub fn set_interval<'js>(
        func: Function<'js>,
        delay: Option<usize>,
        ctx: Ctx<'js>,
    ) -> Result<CancellationTokenWrapper> {
        let delay = delay.unwrap_or(0) as u64;
        let mut interval = time::interval(Duration::from_millis(delay).max(MINIMUM_DELAY));
        interval.set_missed_tick_behavior(time::MissedTickBehavior::Delay);
        let token = CancellationToken::new();

        ctx.spawn({
            let ctx = ctx.clone();
            let token = token.child_token();
            async move {
                // ignore the first tick
                let _ = token.run_until_cancelled(interval.tick()).await;
                while token.run_until_cancelled(interval.tick()).await.is_some() {
                    report_uncaught(&ctx, func.call::<_, ()>(()));
                }
            }
        });

        Ok(token.into())
    }

    #[rquickjs::function(rename = "clearInterval")]
    pub fn clear_interval(token: CancellationTokenWrapper) {
        token.cancel();
    }

    #[rquickjs::function(rename = "setTimeout")]
    pub fn set_timeout<'js>(
        func: Function<'js>,
        delay: Option<usize>,
        ctx: Ctx<'js>,
    ) -> Result<CancellationTokenWrapper> {
        let delay = delay.unwrap_or(0) as u64;
        let duration = Duration::from_millis(delay).max(MINIMUM_DELAY);
        let token = CancellationToken::new();

        ctx.spawn({
            let ctx = ctx.clone();
            let token = token.child_token();
            async move {
                if token
                    .run_until_cancelled(time::sleep(duration))
                    .await
                    .is_some()
                {
                    report_uncaught(&ctx, func.call::<_, ()>(()));
                }
            }
        });
        Ok(token.into())
    }

    #[rquickjs::function(rename = "clearTimeout")]
    pub fn clear_timeout(token: CancellationTokenWrapper) {
        token.cancel();
    }

    #[qjs(declare)]
    pub fn declare(declare: &Declarations) -> Result<()> {
        declare.declare("setInterval")?;
        declare.declare("clearInterval")?;
        declare.declare("setTimeout")?;
        declare.declare("clearTimeout")?;
        Ok(())
    }

    #[qjs(evaluate)]
    pub fn evaluate<'js>(ctx: &Ctx<'js>, _: &Exports<'js>) -> Result<()> {
        ctx.globals().set("setInterval", js_set_interval)?;
        ctx.globals().set("clearInterval", js_clear_interval)?;
        ctx.globals().set("setTimeout", js_set_timeout)?;
        ctx.globals().set("clearTimeout", js_clear_timeout)?;

        Ok(())
    }
}
