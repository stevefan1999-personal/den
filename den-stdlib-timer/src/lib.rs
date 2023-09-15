use std::time::Duration;

use den_stdlib_core::{CancellationTokenWrapper, WORLD_END};
use den_utils::FutureExt;
use rquickjs::{class::Trace, Ctx, Function};
use tokio::time;

#[derive(Trace)]
#[rquickjs::class]
pub struct Timer {}

#[rquickjs::methods(rename_all = "camelCase")]
impl Timer {
    pub fn set_interval<'js>(
        func: Function<'js>,
        delay: Option<usize>,
        ctx: Ctx<'js>,
    ) -> CancellationTokenWrapper {
        let delay = delay.unwrap_or(0) as u64;
        let duration = Duration::from_millis(delay);
        let mut interval = time::interval(duration);
        let token = WORLD_END.child_token();

        ctx.spawn({
            let token = token.clone();
            async move {
                println!("running the interval timer");
                // ignore the first tick
                let _ = interval.tick().with_cancellation(&token).await;
                while let Ok(_) = interval.tick().with_cancellation(&token).await {
                    let _ = func.defer(());
                }
            }
        });

        token.into()
    }

    pub fn clear_interval(token: CancellationTokenWrapper) {
        token.cancel();
    }

    pub fn set_timeout<'js>(
        func: Function<'js>,
        delay: Option<usize>,
        ctx: Ctx<'js>,
    ) -> CancellationTokenWrapper {
        let delay = delay.unwrap_or(0) as u64;
        let duration = Duration::from_millis(delay);
        let token = WORLD_END.child_token();

        ctx.spawn({
            let token = token.clone();
            async move {
                let _ = time::sleep(duration).with_cancellation(&token).await;
                let _ = func.defer(());
            }
        });
        token.into()
    }

    pub fn clear_timeout(token: CancellationTokenWrapper) {
        token.cancel();
    }
}
