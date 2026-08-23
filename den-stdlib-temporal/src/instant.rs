use rquickjs::{
    BigInt, Ctx, JsLifetime, Result, Value, atom::PredefinedAtom, class::Trace, prelude::Opt,
};
use temporal_rs::options::ToStringRoundingOptions;

use crate::convert::{
    bag_value, i128_to_bigint, options_bag, ordering_i32, throw_value_of, to_big_int_i128,
    to_difference_settings, to_duration, to_instant, to_number, to_rounding_options,
    to_string_rounding_options, to_time_zone, unwrap_temporal,
};
use crate::duration::Duration;
use crate::zoned_date_time::ZonedDateTime;

#[derive(Trace, JsLifetime, Clone, Copy)]
#[rquickjs::class(rename = "Instant", frozen)]
pub struct Instant {
    #[qjs(skip_trace)]
    pub(crate) inner: temporal_rs::Instant,
}

impl Instant {
    pub(crate) fn wrap(inner: temporal_rs::Instant) -> Self {
        Self { inner }
    }
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
        epoch_nanoseconds: Value<'js>,
        ctx: Ctx<'js>,
    ) -> Result<Self> {
        let nanoseconds = to_big_int_i128(&ctx, &epoch_nanoseconds)?;
        unwrap_temporal(&ctx, temporal_rs::Instant::try_new(nanoseconds)).map(Self::wrap)
    }

    #[qjs(static)]
    pub fn from_epoch_milliseconds<'js>(
        epoch_milliseconds: Value<'js>,
        ctx: Ctx<'js>,
    ) -> Result<Self> {
        let milliseconds = to_number(&ctx, &epoch_milliseconds)?;
        if !milliseconds.is_finite() || milliseconds.trunc() != milliseconds {
            return Err(rquickjs::Exception::throw_range(
                &ctx,
                "epochMilliseconds must be an integer",
            ));
        }
        unwrap_temporal(
            &ctx,
            temporal_rs::Instant::from_epoch_milliseconds(milliseconds as i64),
        )
        .map(Self::wrap)
    }

    #[qjs(static)]
    pub fn compare<'js>(one: Value<'js>, two: Value<'js>, ctx: Ctx<'js>) -> Result<i32> {
        let left = to_instant(&ctx, &one)?;
        let right = to_instant(&ctx, &two)?;
        Ok(ordering_i32(left.cmp(&right)))
    }

    #[qjs(get)]
    pub fn epoch_nanoseconds<'js>(&self, ctx: Ctx<'js>) -> Result<BigInt<'js>> {
        i128_to_bigint(ctx, self.inner.as_i128())
    }

    #[qjs(get)]
    pub fn epoch_milliseconds(&self) -> i64 {
        self.inner.epoch_milliseconds()
    }

    pub fn add<'js>(&self, duration_like: Value<'js>, ctx: Ctx<'js>) -> Result<Self> {
        let duration = to_duration(&ctx, &duration_like)?;
        unwrap_temporal(&ctx, self.inner.add(&duration)).map(Self::wrap)
    }

    pub fn subtract<'js>(&self, duration_like: Value<'js>, ctx: Ctx<'js>) -> Result<Self> {
        let duration = to_duration(&ctx, &duration_like)?;
        unwrap_temporal(&ctx, self.inner.subtract(&duration)).map(Self::wrap)
    }

    pub fn until<'js>(
        &self,
        other: Value<'js>,
        options: Opt<Value<'js>>,
        ctx: Ctx<'js>,
    ) -> Result<Duration> {
        let other = to_instant(&ctx, &other)?;
        let settings = to_difference_settings(&ctx, &options_bag(&ctx, options)?)?;
        unwrap_temporal(&ctx, self.inner.until(&other, settings)).map(Duration::wrap)
    }

    pub fn since<'js>(
        &self,
        other: Value<'js>,
        options: Opt<Value<'js>>,
        ctx: Ctx<'js>,
    ) -> Result<Duration> {
        let other = to_instant(&ctx, &other)?;
        let settings = to_difference_settings(&ctx, &options_bag(&ctx, options)?)?;
        unwrap_temporal(&ctx, self.inner.since(&other, settings)).map(Duration::wrap)
    }

    pub fn round<'js>(&self, options: Value<'js>, ctx: Ctx<'js>) -> Result<Self> {
        let bag = if options.is_string() {
            let mut map = indexmap::IndexMap::new();
            map.insert("smallestUnit".to_string(), options);
            map
        } else {
            options_bag(&ctx, Opt(Some(options)))?
        };
        let rounding = to_rounding_options(&ctx, &bag)?;
        unwrap_temporal(&ctx, self.inner.round(rounding)).map(Self::wrap)
    }

    pub fn equals<'js>(&self, other: Value<'js>, ctx: Ctx<'js>) -> Result<bool> {
        Ok(self.inner == to_instant(&ctx, &other)?)
    }

    pub fn to_zoned_date_time_iso<'js>(
        &self,
        time_zone: Value<'js>,
        ctx: Ctx<'js>,
    ) -> Result<ZonedDateTime> {
        let zone = to_time_zone(&ctx, &time_zone)?;
        unwrap_temporal(&ctx, self.inner.to_zoned_date_time_iso(zone)).map(ZonedDateTime::wrap)
    }

    pub fn to_string<'js>(&self, options: Opt<Value<'js>>, ctx: Ctx<'js>) -> Result<String> {
        let bag = options_bag(&ctx, options)?;
        let time_zone = match bag_value(&bag, "timeZone") {
            None => None,
            Some(value) => Some(to_time_zone(&ctx, value)?),
        };
        let rounding = to_string_rounding_options(&ctx, &bag)?;
        unwrap_temporal(&ctx, self.inner.to_ixdtf_string(time_zone, rounding))
    }

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
    pub fn to_string_tag() -> &'static str {
        "Temporal.Instant"
    }
}
