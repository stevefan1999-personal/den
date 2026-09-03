use rquickjs::{
    BigInt, Ctx, Exception, JsLifetime, Result, Value, atom::PredefinedAtom, class::Trace,
    prelude::Opt,
};
use temporal_rs::{TimeZone, options::ToStringRoundingOptions};

use crate::{
    convert::{
        difference_settings, get_defined, i128_to_bigint, require_object, rounding_options,
        throw_value_of, to_big_int_i128, to_duration, to_instant, to_integer_if_integral,
        to_string_rounding, to_time_zone, unwrap_temporal,
    },
    duration::Duration,
    zoned_date_time::ZonedDateTime,
};

#[derive(Trace, JsLifetime, Clone, Copy)]
#[rquickjs::class(rename = "Instant", frozen)]
pub struct Instant {
    #[qjs(skip_trace)]
    pub(crate) inner: temporal_rs::Instant,
}

impl Instant {
    pub(crate) const fn wrap(inner: temporal_rs::Instant) -> Self { Self { inner } }
}

/// Instant.toString: Get fractionalSecondDigits, roundingMode, smallestUnit,
/// timeZone.
fn instant_to_string_parts<'js>(
    ctx: &Ctx<'js>, options: Opt<Value<'js>>,
) -> Result<(Option<TimeZone>, ToStringRoundingOptions)> {
    let Some(value) = options.0.filter(|value| !value.is_undefined()) else {
        return Ok((None, ToStringRoundingOptions::default()));
    };
    let object = require_object(ctx, &value, "options must be an object")?;
    let rounding = to_string_rounding(ctx, &object, "fractionalSecondDigits is not finite")?;
    let time_zone = get_defined(&object, "timeZone")?
        .map(|value| to_time_zone(ctx, &value))
        .transpose()?;
    Ok((time_zone, rounding))
}

#[rquickjs::methods(rename_all = "camelCase")]
impl Instant {
    #[qjs(constructor)]
    pub fn new<'js>(epoch_nanoseconds: Value<'js>, ctx: Ctx<'js>) -> Result<Self> {
        let nanoseconds = to_big_int_i128(&ctx, &epoch_nanoseconds)?;
        unwrap_temporal(&ctx, temporal_rs::Instant::try_new(nanoseconds)).map(Self::wrap)
    }

    #[qjs(static)]
    pub fn from<'js>(item: Value<'js>, ctx: Ctx<'js>) -> Result<Self> {
        to_instant(&ctx, &item).map(Self::wrap)
    }

    #[qjs(static)]
    pub fn from_epoch_nanoseconds<'js>(
        epoch_nanoseconds: Value<'js>, ctx: Ctx<'js>,
    ) -> Result<Self> {
        let nanoseconds = to_big_int_i128(&ctx, &epoch_nanoseconds)?;
        unwrap_temporal(&ctx, temporal_rs::Instant::try_new(nanoseconds)).map(Self::wrap)
    }

    #[qjs(static)]
    pub fn from_epoch_milliseconds<'js>(
        epoch_milliseconds: Opt<Value<'js>>, ctx: Ctx<'js>,
    ) -> Result<Self> {
        // NumberToBigInt after ToNumber. Missing/undefined → NaN → RangeError.
        let Some(epoch_milliseconds) = epoch_milliseconds.0.filter(|value| !value.is_undefined())
        else {
            return Err(Exception::throw_range(
                &ctx,
                "epochMilliseconds must be an integer",
            ));
        };
        let milliseconds = to_integer_if_integral(&ctx, &epoch_milliseconds)?;
        let milliseconds = i64::try_from(milliseconds)
            .map_err(|_error| Exception::throw_range(&ctx, "epochMilliseconds is out of range"))?;
        unwrap_temporal(
            &ctx,
            temporal_rs::Instant::from_epoch_milliseconds(milliseconds),
        )
        .map(Self::wrap)
    }

    #[qjs(static)]
    pub fn compare<'js>(one: Value<'js>, two: Value<'js>, ctx: Ctx<'js>) -> Result<i32> {
        let left = to_instant(&ctx, &one)?;
        let right = to_instant(&ctx, &two)?;
        Ok(left.cmp(&right) as i32)
    }

    #[qjs(get)]
    pub fn epoch_nanoseconds<'js>(&self, ctx: Ctx<'js>) -> Result<BigInt<'js>> {
        i128_to_bigint(ctx, self.inner.as_i128())
    }

    #[qjs(get)]
    pub fn epoch_milliseconds(&self) -> i64 { self.inner.epoch_milliseconds() }

    pub fn add<'js>(&self, duration_like: Value<'js>, ctx: Ctx<'js>) -> Result<Self> {
        let duration = to_duration(&ctx, &duration_like)?;
        unwrap_temporal(&ctx, self.inner.add(&duration)).map(Self::wrap)
    }

    pub fn subtract<'js>(&self, duration_like: Value<'js>, ctx: Ctx<'js>) -> Result<Self> {
        let duration = to_duration(&ctx, &duration_like)?;
        unwrap_temporal(&ctx, self.inner.subtract(&duration)).map(Self::wrap)
    }

    pub fn until<'js>(
        &self, other: Value<'js>, options: Opt<Value<'js>>, ctx: Ctx<'js>,
    ) -> Result<Duration> {
        let other = to_instant(&ctx, &other)?;
        let settings = difference_settings(&ctx, options)?;
        unwrap_temporal(&ctx, self.inner.until(&other, settings)).map(Duration::wrap)
    }

    pub fn since<'js>(
        &self, other: Value<'js>, options: Opt<Value<'js>>, ctx: Ctx<'js>,
    ) -> Result<Duration> {
        let other = to_instant(&ctx, &other)?;
        let settings = difference_settings(&ctx, options)?;
        unwrap_temporal(&ctx, self.inner.since(&other, settings)).map(Duration::wrap)
    }

    pub fn round<'js>(&self, options: Value<'js>, ctx: Ctx<'js>) -> Result<Self> {
        let rounding = rounding_options(&ctx, &options)?;
        unwrap_temporal(&ctx, self.inner.round(rounding)).map(Self::wrap)
    }

    pub fn equals<'js>(&self, other: Value<'js>, ctx: Ctx<'js>) -> Result<bool> {
        Ok(self.inner == to_instant(&ctx, &other)?)
    }

    #[qjs(rename = "toZonedDateTimeISO")]
    pub fn to_zoned_date_time_iso<'js>(
        &self, time_zone: Value<'js>, ctx: Ctx<'js>,
    ) -> Result<ZonedDateTime> {
        let zone = to_time_zone(&ctx, &time_zone)?;
        unwrap_temporal(&ctx, self.inner.to_zoned_date_time_iso(zone)).map(ZonedDateTime::wrap)
    }

    pub fn to_string<'js>(&self, options: Opt<Value<'js>>, ctx: Ctx<'js>) -> Result<String> {
        let (time_zone, rounding) = instant_to_string_parts(&ctx, options)?;
        unwrap_temporal(&ctx, self.inner.to_ixdtf_string(time_zone, rounding))
    }

    #[qjs(rename = "toJSON")]
    pub fn to_json(&self, ctx: Ctx<'_>) -> Result<String> {
        unwrap_temporal(
            &ctx,
            self.inner
                .to_ixdtf_string(None, ToStringRoundingOptions::default()),
        )
    }

    pub fn value_of(&self, ctx: Ctx<'_>) -> Result<()> {
        Err(throw_value_of(&ctx, "Temporal.Instant"))
    }

    #[qjs(prop, rename = PredefinedAtom::SymbolToStringTag, configurable)]
    pub const fn to_string_tag() -> &'static str { "Temporal.Instant" }
}
