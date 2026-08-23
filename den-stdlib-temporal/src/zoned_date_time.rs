use rquickjs::{
    BigInt, Ctx, JsLifetime, Result, Value, atom::PredefinedAtom, class::Trace, prelude::Opt,
};
use temporal_rs::options::{
    DisplayCalendar, DisplayOffset, DisplayTimeZone, ToStringRoundingOptions,
};

use crate::convert::{
    bag_overflow, i128_to_bigint, optional_calendar, options_bag, ordering_i32, throw_value_of,
    to_big_int_i128, to_calendar, to_difference_settings, to_duration, to_string_rounding_options,
    to_time_zone, to_zoned_date_time, unwrap_temporal,
};
use crate::duration::Duration;
use crate::instant::Instant;
use crate::plain_date::PlainDate;
use crate::plain_date_time::PlainDateTime;
use crate::plain_time::PlainTime;

#[derive(Trace, JsLifetime, Clone)]
#[rquickjs::class(rename = "ZonedDateTime", frozen)]
pub struct ZonedDateTime {
    #[qjs(skip_trace)]
    pub(crate) inner: temporal_rs::ZonedDateTime,
}

impl ZonedDateTime {
    pub(crate) fn wrap(inner: temporal_rs::ZonedDateTime) -> Self {
        Self { inner }
    }
}

#[rquickjs::methods(rename_all = "camelCase")]
impl ZonedDateTime {
    #[qjs(constructor)]
    pub fn new<'js>(
        epoch_nanoseconds: Value<'js>,
        time_zone: Value<'js>,
        calendar: Opt<Value<'js>>,
        ctx: Ctx<'js>,
    ) -> Result<Self> {
        let nanoseconds = to_big_int_i128(&ctx, &epoch_nanoseconds)?;
        let zone = to_time_zone(&ctx, &time_zone)?;
        let calendar = optional_calendar(&ctx, calendar)?;
        unwrap_temporal(
            &ctx,
            temporal_rs::ZonedDateTime::try_new(nanoseconds, zone, calendar),
        )
        .map(Self::wrap)
    }

    #[qjs(static)]
    pub fn from<'js>(item: Value<'js>, ctx: Ctx<'js>) -> Result<Self> {
        to_zoned_date_time(&ctx, &item).map(Self::wrap)
    }

    #[qjs(static)]
    pub fn compare<'js>(one: Value<'js>, two: Value<'js>, ctx: Ctx<'js>) -> Result<i32> {
        let left = to_zoned_date_time(&ctx, &one)?;
        let right = to_zoned_date_time(&ctx, &two)?;
        Ok(ordering_i32(left.compare_instant(&right)))
    }

    #[qjs(get)]
    pub fn calendar_id(&self) -> &'static str {
        self.inner.calendar().identifier()
    }

    #[qjs(get)]
    pub fn time_zone_id(&self, ctx: Ctx<'_>) -> Result<String> {
        unwrap_temporal(&ctx, self.inner.time_zone().identifier())
    }

    #[qjs(get)]
    pub fn epoch_nanoseconds<'js>(&self, ctx: Ctx<'js>) -> Result<BigInt<'js>> {
        i128_to_bigint(ctx, self.inner.epoch_nanoseconds().as_i128())
    }

    #[qjs(get)]
    pub fn epoch_milliseconds(&self) -> i64 {
        self.inner.epoch_milliseconds()
    }

    #[qjs(get)]
    pub fn year(&self) -> i32 {
        self.inner.year()
    }

    #[qjs(get)]
    pub fn month(&self) -> u8 {
        self.inner.month()
    }

    #[qjs(get)]
    pub fn month_code(&self) -> String {
        self.inner.month_code().as_str().to_string()
    }

    #[qjs(get)]
    pub fn day(&self) -> u8 {
        self.inner.day()
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

    #[qjs(get)]
    pub fn day_of_week(&self) -> u16 {
        self.inner.day_of_week()
    }

    #[qjs(get)]
    pub fn day_of_year(&self) -> u16 {
        self.inner.day_of_year()
    }

    #[qjs(get)]
    pub fn week_of_year(&self) -> Option<u8> {
        self.inner.week_of_year()
    }

    #[qjs(get)]
    pub fn year_of_week(&self) -> Option<i32> {
        self.inner.year_of_week()
    }

    #[qjs(get)]
    pub fn days_in_week(&self) -> u16 {
        self.inner.days_in_week()
    }

    #[qjs(get)]
    pub fn days_in_month(&self) -> u16 {
        self.inner.days_in_month()
    }

    #[qjs(get)]
    pub fn days_in_year(&self) -> u16 {
        self.inner.days_in_year()
    }

    #[qjs(get)]
    pub fn months_in_year(&self) -> u16 {
        self.inner.months_in_year()
    }

    #[qjs(get)]
    pub fn in_leap_year(&self) -> bool {
        self.inner.in_leap_year()
    }

    #[qjs(get)]
    pub fn offset(&self) -> String {
        self.inner.offset()
    }

    #[qjs(get)]
    pub fn offset_nanoseconds(&self) -> i64 {
        self.inner.offset_nanoseconds()
    }

    #[qjs(get)]
    pub fn hours_in_day(&self, ctx: Ctx<'_>) -> Result<f64> {
        unwrap_temporal(&ctx, self.inner.hours_in_day())
    }

    pub fn add<'js>(
        &self,
        duration_like: Value<'js>,
        options: Opt<Value<'js>>,
        ctx: Ctx<'js>,
    ) -> Result<Self> {
        let duration = to_duration(&ctx, &duration_like)?;
        let overflow = bag_overflow(&ctx, &options_bag(&ctx, options)?)?;
        unwrap_temporal(&ctx, self.inner.add(&duration, overflow)).map(Self::wrap)
    }

    pub fn subtract<'js>(
        &self,
        duration_like: Value<'js>,
        options: Opt<Value<'js>>,
        ctx: Ctx<'js>,
    ) -> Result<Self> {
        let duration = to_duration(&ctx, &duration_like)?;
        let overflow = bag_overflow(&ctx, &options_bag(&ctx, options)?)?;
        unwrap_temporal(&ctx, self.inner.subtract(&duration, overflow)).map(Self::wrap)
    }

    pub fn until<'js>(
        &self,
        other: Value<'js>,
        options: Opt<Value<'js>>,
        ctx: Ctx<'js>,
    ) -> Result<Duration> {
        let other = to_zoned_date_time(&ctx, &other)?;
        let settings = to_difference_settings(&ctx, &options_bag(&ctx, options)?)?;
        unwrap_temporal(&ctx, self.inner.until(&other, settings)).map(Duration::wrap)
    }

    pub fn since<'js>(
        &self,
        other: Value<'js>,
        options: Opt<Value<'js>>,
        ctx: Ctx<'js>,
    ) -> Result<Duration> {
        let other = to_zoned_date_time(&ctx, &other)?;
        let settings = to_difference_settings(&ctx, &options_bag(&ctx, options)?)?;
        unwrap_temporal(&ctx, self.inner.since(&other, settings)).map(Duration::wrap)
    }

    pub fn equals<'js>(&self, other: Value<'js>, ctx: Ctx<'js>) -> Result<bool> {
        let other = to_zoned_date_time(&ctx, &other)?;
        unwrap_temporal(&ctx, self.inner.equals(&other))
    }

    pub fn with_calendar<'js>(&self, calendar: Value<'js>, ctx: Ctx<'js>) -> Result<Self> {
        Ok(Self::wrap(
            self.inner.with_calendar(to_calendar(&ctx, &calendar)?),
        ))
    }

    pub fn with_time_zone<'js>(&self, time_zone: Value<'js>, ctx: Ctx<'js>) -> Result<Self> {
        let zone = to_time_zone(&ctx, &time_zone)?;
        unwrap_temporal(&ctx, self.inner.with_timezone(zone)).map(Self::wrap)
    }

    pub fn to_instant(&self) -> Instant {
        Instant::wrap(self.inner.to_instant())
    }

    pub fn to_plain_date(&self) -> PlainDate {
        PlainDate::wrap(self.inner.to_plain_date())
    }

    pub fn to_plain_time(&self) -> PlainTime {
        PlainTime::wrap(self.inner.to_plain_time())
    }

    pub fn to_plain_date_time(&self) -> PlainDateTime {
        PlainDateTime::wrap(self.inner.to_plain_date_time())
    }

    pub fn start_of_day(&self, ctx: Ctx<'_>) -> Result<Self> {
        unwrap_temporal(&ctx, self.inner.start_of_day()).map(Self::wrap)
    }

    pub fn to_string<'js>(&self, options: Opt<Value<'js>>, ctx: Ctx<'js>) -> Result<String> {
        let rounding = to_string_rounding_options(&ctx, &options_bag(&ctx, options)?)?;
        unwrap_temporal(
            &ctx,
            self.inner.to_ixdtf_string(
                DisplayOffset::Auto,
                DisplayTimeZone::Auto,
                DisplayCalendar::Auto,
                rounding,
            ),
        )
    }

    pub fn to_json(&self, ctx: Ctx<'_>) -> Result<String> {
        unwrap_temporal(
            &ctx,
            self.inner.to_ixdtf_string(
                DisplayOffset::Auto,
                DisplayTimeZone::Auto,
                DisplayCalendar::Auto,
                ToStringRoundingOptions::default(),
            ),
        )
    }

    pub fn value_of(&self, ctx: Ctx<'_>) -> Result<()> {
        Err(throw_value_of(&ctx, "Temporal.ZonedDateTime"))
    }

    #[qjs(prop, rename = PredefinedAtom::SymbolToStringTag, configurable)]
    pub fn to_string_tag() -> &'static str {
        "Temporal.ZonedDateTime"
    }
}
