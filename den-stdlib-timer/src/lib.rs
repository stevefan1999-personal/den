#[rquickjs::module(rename_types = "camelCase", rename = "camelCase")]
pub mod timer {
    use std::time::Duration;

    use den_stdlib_core::{CancellationTokenWrapper, WORLD_END};
    use den_utils::FutureExt;
    use rquickjs::{module::Exports, Ctx, Function};
    use tokio::time;

    #[rquickjs::function(rename = "setInterval")]
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
                // ignore the first tick
                let _ = interval.tick().with_cancellation(&token).await;
                while let Ok(_) = interval.tick().with_cancellation(&token).await {
                    func.call::<_, ()>(()).unwrap();
                }
            }
        });

        token.into()
    }

    #[rquickjs::function(rename = "clearInterval")]
    pub fn clear_interval(token: CancellationTokenWrapper) {
        println!("cancel");
        token.cancel();
    }

    #[rquickjs::function(rename = "setTimeout")]
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
                func.call::<_, ()>(()).unwrap();
            }
        });
        token.into()
    }

    #[rquickjs::function(rename = "clearTimeout")]
    pub fn clear_timeout(token: CancellationTokenWrapper) {
        token.cancel();
    }

    #[qjs(declare)]
    pub fn declare(declare: &mut rquickjs::module::Declarations) -> rquickjs::Result<()> {
        declare.declare("setInterval")?;
        declare.declare("clearInterval")?;
        declare.declare("setTimeout")?;
        declare.declare("clearTimeout")?;
        Ok(())
    }

    #[qjs(evaluate)]
    pub fn evaluate<'js>(ctx: &Ctx<'js>, exports: &mut Exports<'js>) -> rquickjs::Result<()> {
        for (k, v) in exports.iter() {
            ctx.globals().set(k.to_str()?, v)?;
        }

        Ok(())
    }
}
