#[rquickjs::module(rename_types = "camelCase", rename = "camelCase")]
pub mod timer {
    use std::time::Duration;

    use den_stdlib_core::{CancellationToken, CancellationTokenWrapper};
    use den_utils::FutureExt;
    use rquickjs::{module::Exports, Ctx, Function};
    use tokio::time;

    #[rquickjs::function(rename = "setInterval")]
    pub fn set_interval<'js>(
        func: Function<'js>,
        delay: Option<usize>,
        ctx: Ctx<'js>,
    ) -> rquickjs::Result<CancellationTokenWrapper> {
        let delay = delay.unwrap_or(0) as u64;
        let duration = Duration::from_millis(delay);
        let mut interval = time::interval(duration);
        interval.set_missed_tick_behavior(time::MissedTickBehavior::Delay);
        let token = CancellationToken::new();

        ctx.spawn({
            let token = token.child_token();
            async move {
                // ignore the first tick
                let _ = interval.tick().with_cancellation(&token).await;
                while let Ok(_) = interval.tick().with_cancellation(&token).await {
                    let _ = func.call::<_, ()>(());
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
    ) -> rquickjs::Result<CancellationTokenWrapper> {
        let delay = delay.unwrap_or(0) as u64;
        let duration: Duration = Duration::from_millis(delay);
        let token = CancellationToken::new();

        ctx.spawn({
            let token = token.child_token();
            async move {
                if let Ok(()) = time::sleep(duration).with_cancellation(&token).await {
                    let _ = func.call::<_, ()>(());
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
    pub fn declare(declare: &rquickjs::module::Declarations) -> rquickjs::Result<()> {
        declare.declare("setInterval")?;
        declare.declare("clearInterval")?;
        declare.declare("setTimeout")?;
        declare.declare("clearTimeout")?;
        Ok(())
    }

    #[qjs(evaluate)]
    pub fn evaluate<'js>(ctx: &Ctx<'js>, _: &Exports<'js>) -> rquickjs::Result<()> {
        ctx.globals().set("setInterval", js_set_interval)?;
        ctx.globals().set("clearInterval", js_clear_interval)?;
        ctx.globals().set("setTimeout", js_set_timeout)?;
        ctx.globals().set("clearTimeout", js_clear_timeout)?;

        Ok(())
    }
}
