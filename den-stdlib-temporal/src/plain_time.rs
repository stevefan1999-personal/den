use rquickjs::{Ctx, JsLifetime, Result, Value, atom::PredefinedAtom, class::Trace, prelude::Opt};
use temporal_rs::options::ToStringRoundingOptions;

use crate::convert::{
    bag_overflow, optional_truncated_u8, optional_truncated_u16, options_bag, ordering_i32,
    throw_value_of, to_difference_settings, to_duration, to_plain_time, to_string_rounding_options,
    unwrap_temporal,
};
use crate::duration::Duration;

#[derive(Trace, JsLifetime, Clone, Copy)]
#[rquickjs::class(rename = "PlainTime", frozen)]
pub struct PlainTime {
    #[qjs(skip_trace)]
    pub(crate) inner: temporal_rs::PlainTime,
}

impl PlainTime {
    pub(crate) fn wrap(inner: temporal_rs::PlainTime) -> Self {
        Self { inner }
    }
}

#[rquickjs::methods(rename_all = "camelCase")]
impl PlainTime {
    #[qjs(constructor)]
    pub fn new<'js>(
        hour: Opt<Value<'js>>,
        minute: Opt<Value<'js>>,
        second: Opt<Value<'js>>,
        millisecond: Opt<Value<'js>>,
        microsecond: Opt<Value<'js>>,
        nanosecond: Opt<Value<'js>>,
        ctx: Ctx<'js>,
    ) -> Result<Self> {
        unwrap_temporal(
            &ctx,
            temporal_rs::PlainTime::try_new(
                optional_truncated_u8(&ctx, hour)?,
                optional_truncated_u8(&ctx, minute)?,
                optional_truncated_u8(&ctx, second)?,
                optional_truncated_u16(&ctx, millisecond)?,
                optional_truncated_u16(&ctx, microsecond)?,
                optional_truncated_u16(&ctx, nanosecond)?,
            ),
        )
        .map(Self::wrap)
    }

    #[qjs(static)]
    pub fn from<'js>(item: Value<'js>, options: Opt<Value<'js>>, ctx: Ctx<'js>) -> Result<Self> {
        let overflow = bag_overflow(&ctx, &options_bag(&ctx, options)?)?;
        to_plain_time(&ctx, &item, overflow).map(Self::wrap)
    }

    #[qjs(static)]
    pub fn compare<'js>(one: Value<'js>, two: Value<'js>, ctx: Ctx<'js>) -> Result<i32> {
        let left = to_plain_time(&ctx, &one, None)?;
        let right = to_plain_time(&ctx, &two, None)?;
        Ok(ordering_i32(left.cmp(&right)))
    }

    #[qjs(get)]
    pub fn hour(&self) -> u8 {
        self.inner.hour()
    }

    #[qjs(get)]
    pub fn minute(&self) -> u8 {
        self.inner.minute()
    }

    #[qjs(get)]
    pub fn second(&self) -> u8 {
        self.inner.second()
    }

    #[qjs(get)]
    pub fn millisecond(&self) -> u16 {
        self.inner.millisecond()
    }

    #[qjs(get)]
    pub fn microsecond(&self) -> u16 {
        self.inner.microsecond()
    }

    #[qjs(get)]
    pub fn nanosecond(&self) -> u16 {
        self.inner.nanosecond()
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
        let other = to_plain_time(&ctx, &other, None)?;
        let settings = to_difference_settings(&ctx, &options_bag(&ctx, options)?)?;
        unwrap_temporal(&ctx, self.inner.until(&other, settings)).map(Duration::wrap)
    }

    pub fn since<'js>(
        &self,
        other: Value<'js>,
        options: Opt<Value<'js>>,
        ctx: Ctx<'js>,
    ) -> Result<Duration> {
        let other = to_plain_time(&ctx, &other, None)?;
        let settings = to_difference_settings(&ctx, &options_bag(&ctx, options)?)?;
        unwrap_temporal(&ctx, self.inner.since(&other, settings)).map(Duration::wrap)
    }

    pub fn to_string<'js>(&self, options: Opt<Value<'js>>, ctx: Ctx<'js>) -> Result<String> {
        let rounding = to_string_rounding_options(&ctx, &options_bag(&ctx, options)?)?;
        unwrap_temporal(&ctx, self.inner.to_ixdtf_string(rounding))
    }

    pub fn to_json(&self, ctx: Ctx<'_>) -> Result<String> {
        unwrap_temporal(
            &ctx,
            self.inner
                .to_ixdtf_string(ToStringRoundingOptions::default()),
        )
    }

    pub fn value_of(&self, ctx: Ctx<'_>) -> Result<()> {
        Err(throw_value_of(&ctx, "Temporal.PlainTime"))
    }

    #[qjs(prop, rename = PredefinedAtom::SymbolToStringTag, configurable)]
    pub fn to_string_tag() -> &'static str {
        "Temporal.PlainTime"
    }
}
