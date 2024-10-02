use derivative::Derivative;
use derive_more::{From, Into};
use quanta::{Clock, Instant};
use rquickjs::{class::Trace, Coerced, Ctx, Exception, Result};

pub use crate::cancellation::CancellationTokenWrapper;

#[derive(Trace, Derivative, From, Into)]
#[derivative(Clone, Debug)]
#[rquickjs::class(rename = "Performance")]
pub struct Performance {
    #[qjs(skip_trace)]
    clock:   Clock,
    #[qjs(skip_trace)]
    instant: Instant,
}

#[rquickjs::methods]
impl Performance {
    #[qjs(constructor)]
    pub fn new() -> Result<Self> {
        let clock = Clock::new();
        let instant = clock.now();
        Ok(Self { clock, instant })
    }

    pub fn now(self) -> u64 {
        self.instant.elapsed().as_millis().try_into().unwrap()
    }

    #[qjs(get, enumerable, rename = "timeOrigin")]
    pub fn time_origin(self) -> u64 {
        self.clock.raw()
    }
}

#[rquickjs::function()]
pub fn btoa(value: Coerced<String>) -> Result<String> {
    #[cfg(feature = "base64-simd")]
    {
        use base64_simd::STANDARD;
        Ok(STANDARD.encode_to_string(value.as_bytes()))
    }
    #[cfg(feature = "base64")]
    {
        use base64::prelude::*;

        Ok(BASE64_STANDARD.encode(value.as_bytes()))
    }
}

#[rquickjs::function()]
pub fn atob(ctx: Ctx<'_>, value: Coerced<String>) -> Result<String> {
    #[cfg(feature = "base64-simd")]
    {
        use base64_simd::STANDARD;
        match STANDARD.decode_to_vec(value.as_bytes()) {
            Ok(decoded) => Ok(String::from_utf8(decoded)?),
            Err(e) => Err(Exception::throw_internal(&ctx, &format!("{e}"))),
        }
    }
    #[cfg(feature = "base64")]
    {
        use base64::prelude::*;
        match BASE64_STANDARD.decode(value.as_bytes()) {
            Ok(decoded) => Ok(String::from_utf8(decoded)?),
            Err(e) => Err(Exception::throw_internal(&ctx, &format!("{e}"))),
        }
    }
}

#[rquickjs::function()]
pub fn gc<'js>(ctx: Ctx<'js>) {
    ctx.run_gc();
}

#[rquickjs::module(rename = "camelCase", rename_vars = "camelCase")]
pub mod core {
    use rquickjs::{
        module::{Declarations, Exports},
        Ctx, Result,
    };

    pub use crate::{cancellation::CancellationTokenWrapper, Performance};

    #[qjs(declare)]
    pub fn declare(declare: &Declarations) -> Result<()> {
        declare
            .declare("atob")?
            .declare("btoa")?
            .declare("performance")?
            .declare("gc")?;
        Ok(())
    }

    #[qjs(evaluate)]
    pub fn evaluate<'js>(ctx: &Ctx<'js>, e: &Exports<'js>) -> Result<()> {
        let performance = Performance::new()?;

        e.export("atob", super::js_atob)?
            .export("btoa", super::js_btoa)?
            .export("performance", performance.clone())?
            .export("gc", super::js_gc)?;

        ctx.globals().set("atob", super::js_atob)?;
        ctx.globals().set("btoa", super::js_btoa)?;
        ctx.globals().set("performance", performance)?;
        ctx.globals().set("gc", super::js_gc)?;
        Ok(())
    }
}

pub mod cancellation;
