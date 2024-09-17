use derivative::Derivative;
use derive_more::{From, Into};
use quanta::{Clock, Instant};
use rquickjs::class::Trace;
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
    pub fn new() -> rquickjs::Result<Self> {
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

#[rquickjs::module(rename = "camelCase", rename_vars = "camelCase")]
pub mod core {
    use rquickjs::{module::Exports, Coerced, Ctx};

    pub use crate::cancellation::CancellationTokenWrapper;
    use crate::Performance;

    #[rquickjs::function()]
    pub fn btoa(value: Coerced<String>) -> rquickjs::Result<String> {
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
    pub fn atob<'js>(ctx: Ctx<'js>, value: Coerced<String>) -> rquickjs::Result<String> {
        #[cfg(feature = "base64-simd")]
        {
            use base64_simd::STANDARD;
            match STANDARD.decode_to_vec(value.as_bytes()) 
            {
                Ok(decoded) => Ok(String::from_utf8(decoded)?),
                Err(e) => Err(rquickjs::Exception::throw_internal(&ctx, &format!("{e}"))),
            }
        }
        #[cfg(feature = "base64")]
        {
            use base64::prelude::*;
            match BASE64_STANDARD.decode(value.as_bytes()) 
            {
                Ok(decoded) => Ok(String::from_utf8(decoded)?),
                Err(e) => Err(rquickjs::Exception::throw_internal(&ctx, &format!("{e}"))),
            }
        }

    }

    #[qjs(declare)]
    pub fn declare(declare: &rquickjs::module::Declarations) -> rquickjs::Result<()> {
        declare.declare("atob")?;
        declare.declare("btoa")?;
        declare.declare("performance")?;
        Ok(())
    }

    #[qjs(evaluate)]
    pub fn evaluate<'js>(ctx: &Ctx<'js>, _: &Exports<'js>) -> rquickjs::Result<()> {
        ctx.globals().set("atob", js_atob)?;
        ctx.globals().set("btoa", js_btoa)?;
        ctx.globals().set("performance", Performance::new())?;
        Ok(())
    }
}

pub mod cancellation;
