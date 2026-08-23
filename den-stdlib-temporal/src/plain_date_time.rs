use rquickjs::{Ctx, JsLifetime, Result, Value, atom::PredefinedAtom, class::Trace, prelude::Opt};
use temporal_rs::options::{DisplayCalendar, ToStringRoundingOptions};

use crate::convert::{
    bag_overflow, bag_value, optional_calendar, optional_truncated_u8, optional_truncated_u16,
    options_bag, ordering_i32, required_i32, required_u8, throw_value_of, to_calendar,
    to_difference_settings, to_display_calendar, to_duration, to_plain_date_time,
    to_string_rounding_options, to_time_zone, unwrap_temporal,
};
use crate::duration::Duration;
use crate::plain_date::PlainDate;
use crate::plain_time::PlainTime;
use crate::zoned_date_time::ZonedDateTime;

#[derive(Trace, JsLifetime, Clone)]
#[rquickjs::class(rename = "PlainDateTime", frozen)]
pub struct PlainDateTime {
    #[qjs(skip_trace)]
    pub(crate) inner: temporal_rs::PlainDateTime,
}

impl PlainDateTime {
    pub(crate) fn wrap(inner: temporal_rs::PlainDateTime) -> Self {
        Self { inner }
    }
}

#[rquickjs::methods(rename_all = "camelCase")]
impl PlainDateTime {
    #[qjs(constructor)]
    #[allow(clippy::too_many_arguments)]
    pub fn new<'js>(
        iso_year: Opt<Value<'js>>,
        iso_month: Opt<Value<'js>>,
        iso_day: Opt<Value<'js>>,
        hour: Opt<Value<'js>>,
        minute: Opt<Value<'js>>,
        second: Opt<Value<'js>>,
        millisecond: Opt<Value<'js>>,
        microsecond: Opt<Value<'js>>,
        nanosecond: Opt<Value<'js>>,
        calendar: Opt<Value<'js>>,
        ctx: Ctx<'js>,
    ) -> Result<Self> {
        unwrap_temporal(
            &ctx,
            temporal_rs::PlainDateTime::try_new(
                required_i32(&ctx, iso_year, "year")?,
                required_u8(&ctx, iso_month, "month")?,
                required_u8(&ctx, iso_day, "day")?,
                optional_truncated_u8(&ctx, hour)?,
                optional_truncated_u8(&ctx, minute)?,
                optional_truncated_u8(&ctx, second)?,
                optional_truncated_u16(&ctx, millisecond)?,
                optional_truncated_u16(&ctx, microsecond)?,
                optional_truncated_u16(&ctx, nanosecond)?,
                optional_calendar(&ctx, calendar)?,
            ),
        )
        .map(Self::wrap)
    }

    #[qjs(static)]
    pub fn from<'js>(item: Value<'js>, ctx: Ctx<'js>) -> Result<Self> {
        to_plain_date_time(&ctx, &item).map(Self::wrap)
    }

    #[qjs(static)]
    pub fn compare<'js>(one: Value<'js>, two: Value<'js>, ctx: Ctx<'js>) -> Result<i32> {
        let left = to_plain_date_time(&ctx, &one)?;
        let right = to_plain_date_time(&ctx, &two)?;
        Ok(ordering_i32(left.compare_iso(&right)))
    }

    #[qjs(get)]
    pub fn calendar_id(&self) -> &'static str {
        self.inner.calendar().identifier()
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
        let other = to_plain_date_time(&ctx, &other)?;
        let settings = to_difference_settings(&ctx, &options_bag(&ctx, options)?)?;
        unwrap_temporal(&ctx, self.inner.until(&other, settings)).map(Duration::wrap)
    }

    pub fn since<'js>(
        &self,
        other: Value<'js>,
        options: Opt<Value<'js>>,
        ctx: Ctx<'js>,
    ) -> Result<Duration> {
        let other = to_plain_date_time(&ctx, &other)?;
        let settings = to_difference_settings(&ctx, &options_bag(&ctx, options)?)?;
        unwrap_temporal(&ctx, self.inner.since(&other, settings)).map(Duration::wrap)
    }

    pub fn with_calendar<'js>(&self, calendar: Value<'js>, ctx: Ctx<'js>) -> Result<Self> {
        Ok(Self::wrap(
            self.inner.with_calendar(to_calendar(&ctx, &calendar)?),
        ))
    }

    pub fn to_plain_date(&self) -> PlainDate {
        PlainDate::wrap(self.inner.to_plain_date())
    }

    pub fn to_plain_time(&self) -> PlainTime {
        PlainTime::wrap(self.inner.to_plain_time())
    }

    pub fn to_zoned_date_time<'js>(
        &self,
        time_zone: Value<'js>,
        options: Opt<Value<'js>>,
        ctx: Ctx<'js>,
    ) -> Result<ZonedDateTime> {
        let zone = to_time_zone(&ctx, &time_zone)?;
        let disambiguation = match bag_value(&options_bag(&ctx, options)?, "disambiguation") {
            None => temporal_rs::options::Disambiguation::Compatible,
            Some(value) => {
                let name = crate::convert::js_to_string(&ctx, value)?;
                std::str::FromStr::from_str(&name)
                    .map_err(|_| rquickjs::Exception::throw_range(&ctx, "invalid disambiguation"))?
            }
        };
        unwrap_temporal(&ctx, self.inner.to_zoned_date_time(zone, disambiguation))
            .map(ZonedDateTime::wrap)
    }

    pub fn to_string<'js>(&self, options: Opt<Value<'js>>, ctx: Ctx<'js>) -> Result<String> {
        let bag = options_bag(&ctx, options)?;
        let rounding = to_string_rounding_options(&ctx, &bag)?;
        let display = match bag_value(&bag, "calendarName") {
            None => DisplayCalendar::Auto,
            Some(value) => to_display_calendar(&ctx, value)?,
        };
        unwrap_temporal(&ctx, self.inner.to_ixdtf_string(rounding, display))
    }

    pub fn to_json(&self, ctx: Ctx<'_>) -> Result<String> {
        unwrap_temporal(
            &ctx,
            self.inner
                .to_ixdtf_string(ToStringRoundingOptions::default(), DisplayCalendar::Auto),
        )
    }

    pub fn value_of(&self, ctx: Ctx<'_>) -> Result<()> {
        Err(throw_value_of(&ctx, "Temporal.PlainDateTime"))
    }

    #[qjs(prop, rename = PredefinedAtom::SymbolToStringTag, configurable)]
    pub fn to_string_tag() -> &'static str {
        "Temporal.PlainDateTime"
    }
}
