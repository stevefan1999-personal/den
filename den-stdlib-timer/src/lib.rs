use rquickjs::bind;

#[bind(object, public)]
#[quickjs(bare)]
pub mod timer {
    use std::time::Duration;

    use den_stdlib_core::{cancellation_token::CancellationTokenWrapper, WORLD_END};
    use den_utils::FutureExt;
    use rquickjs::{Context, Ctx, Function, Persistent};
    use tokio::time;

    #[quickjs(rename = "setInterval")]
    pub fn set_interval(
        func: Persistent<Function<'static>>,
        delay: Option<usize>,
        ctx: Ctx,
    ) -> CancellationTokenWrapper {
        let delay = delay.unwrap_or(0) as u64;
        let duration = Duration::from_millis(delay);
        let mut interval = time::interval(duration);
        let token = WORLD_END.child_token();

        ctx.spawn({
            let token = token.clone();
            let context = Context::from_ctx(ctx).unwrap();
            async move {
                // ignore the first tick
                let _ = interval.tick().with_cancellation(&token).await;
                while let Ok(_) = interval
                    .tick()
                    .with_cancellation(&token)
                    .await
                    .map(|_| context.with(|ctx| func.clone().restore(ctx)?.defer_call(())))
                {
                }
            }
        });

        token.into()
    }

    #[quickjs(rename = "clearInterval")]
    pub fn clear_interval(token: CancellationTokenWrapper) {
        token.cancel();
    }

    #[quickjs(rename = "setTimeout")]
    pub fn set_timeout(
        func: Persistent<Function<'static>>,
        delay: Option<usize>,
        ctx: Ctx,
    ) -> CancellationTokenWrapper {
        let delay = delay.unwrap_or(0) as u64;
        let duration = Duration::from_millis(delay);
        let token = WORLD_END.child_token();
        ctx.spawn({
            let token = token.clone();
            let context = Context::from_ctx(ctx).unwrap();
            async move {
                let _ = time::sleep(duration)
                    .with_cancellation(&token)
                    .await
                    .map(|_| context.with(|ctx| func.restore(ctx)?.defer_call(())));
            }
        });
        token.into()
    }

    #[quickjs(rename = "clearTimeout")]
    pub fn clear_timeout(token: CancellationTokenWrapper) {
        token.cancel();
    }
}
