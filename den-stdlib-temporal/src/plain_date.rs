use rquickjs::{
    Ctx, FromJs, JsLifetime, Result, Value, atom::PredefinedAtom, class::Trace, prelude::Opt,
};
use temporal_rs::options::DisplayCalendar;

use crate::convert::{
    bag_overflow, bag_value, optional_calendar, options_bag, ordering_i32, required_i32,
    required_u8, throw_value_of, to_calendar, to_difference_settings, to_display_calendar,
    to_duration, to_plain_date, to_time_zone, unwrap_temporal,
};
use crate::duration::Duration;
use crate::plain_date_time::PlainDateTime;
use crate::plain_month_day::PlainMonthDay;
use crate::plain_year_month::PlainYearMonth;
use crate::zoned_date_time::ZonedDateTime;

#[derive(Trace, JsLifetime, Clone)]
#[rquickjs::class(rename = "PlainDate", frozen)]
pub struct PlainDate {
    #[qjs(skip_trace)]
    pub(crate) inner: temporal_rs::PlainDate,
}

impl PlainDate {
    pub(crate) fn wrap(inner: temporal_rs::PlainDate) -> Self {
        Self { inner }
    }
}

#[rquickjs::methods(rename_all = "camelCase")]
impl PlainDate {
    #[qjs(constructor)]
    pub fn new<'js>(
        iso_year: Opt<Value<'js>>,
        iso_month: Opt<Value<'js>>,
        iso_day: Opt<Value<'js>>,
        calendar: Opt<Value<'js>>,
        ctx: Ctx<'js>,
    ) -> Result<Self> {
        let year = required_i32(&ctx, iso_year, "year")?;
        let month = required_u8(&ctx, iso_month, "month")?;
        let day = required_u8(&ctx, iso_day, "day")?;
        let calendar = optional_calendar(&ctx, calendar)?;
        unwrap_temporal(
            &ctx,
            temporal_rs::PlainDate::try_new(year, month, day, calendar),
        )
        .map(Self::wrap)
    }

    #[qjs(static)]
    pub fn from<'js>(item: Value<'js>, options: Opt<Value<'js>>, ctx: Ctx<'js>) -> Result<Self> {
        let overflow = bag_overflow(&ctx, &options_bag(&ctx, options)?)?;
        to_plain_date(&ctx, &item, overflow).map(Self::wrap)
    }

    #[qjs(static)]
    pub fn compare<'js>(one: Value<'js>, two: Value<'js>, ctx: Ctx<'js>) -> Result<i32> {
        let left = to_plain_date(&ctx, &one, None)?;
        let right = to_plain_date(&ctx, &two, None)?;
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
    pub fn era(&self) -> Option<String> {
        self.inner.era().map(|era| era.to_string())
    }

    #[qjs(get)]
    pub fn era_year(&self) -> Option<i32> {
        self.inner.era_year()
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
        let other = to_plain_date(&ctx, &other, None)?;
        let settings = to_difference_settings(&ctx, &options_bag(&ctx, options)?)?;
        unwrap_temporal(&ctx, self.inner.until(&other, settings)).map(Duration::wrap)
    }

    pub fn since<'js>(
        &self,
        other: Value<'js>,
        options: Opt<Value<'js>>,
        ctx: Ctx<'js>,
    ) -> Result<Duration> {
        let other = to_plain_date(&ctx, &other, None)?;
        let settings = to_difference_settings(&ctx, &options_bag(&ctx, options)?)?;
        unwrap_temporal(&ctx, self.inner.since(&other, settings)).map(Duration::wrap)
    }

    pub fn with_calendar<'js>(&self, calendar: Value<'js>, ctx: Ctx<'js>) -> Result<Self> {
        Ok(Self::wrap(
            self.inner.with_calendar(to_calendar(&ctx, &calendar)?),
        ))
    }

    pub fn to_plain_date_time<'js>(
        &self,
        time: Opt<Value<'js>>,
        ctx: Ctx<'js>,
    ) -> Result<PlainDateTime> {
        let time = match time.0 {
            None => None,
            Some(value) if value.is_undefined() => None,
            Some(value) => Some(crate::convert::to_plain_time(&ctx, &value, None)?),
        };
        unwrap_temporal(&ctx, self.inner.to_plain_date_time(time)).map(PlainDateTime::wrap)
    }

    pub fn to_plain_year_month(&self, ctx: Ctx<'_>) -> Result<PlainYearMonth> {
        unwrap_temporal(&ctx, self.inner.to_plain_year_month()).map(PlainYearMonth::wrap)
    }

    pub fn to_plain_month_day(&self, ctx: Ctx<'_>) -> Result<PlainMonthDay> {
        unwrap_temporal(&ctx, self.inner.to_plain_month_day()).map(PlainMonthDay::wrap)
    }

    pub fn to_zoned_date_time<'js>(
        &self,
        item: Value<'js>,
        ctx: Ctx<'js>,
    ) -> Result<ZonedDateTime> {
        let (time_zone, plain_time) = if item.is_string() {
            (to_time_zone(&ctx, &item)?, None)
        } else if item.is_object() {
            let bag = indexmap::IndexMap::from_js(&ctx, item)?;
            let zone = match bag_value(&bag, "timeZone") {
                Some(value) => to_time_zone(&ctx, value)?,
                None => {
                    return Err(rquickjs::Exception::throw_type(
                        &ctx,
                        "timeZone is required",
                    ));
                }
            };
            let time = match bag_value(&bag, "plainTime") {
                Some(value) => Some(crate::convert::to_plain_time(&ctx, value, None)?),
                None => None,
            };
            (zone, time)
        } else {
            return Err(rquickjs::Exception::throw_type(
                &ctx,
                "timeZone is required",
            ));
        };
        unwrap_temporal(&ctx, self.inner.to_zoned_date_time(time_zone, plain_time))
            .map(ZonedDateTime::wrap)
    }

    pub fn to_string<'js>(&self, options: Opt<Value<'js>>, ctx: Ctx<'js>) -> Result<String> {
        let display = match bag_value(&options_bag(&ctx, options)?, "calendarName") {
            None => DisplayCalendar::Auto,
            Some(value) => to_display_calendar(&ctx, value)?,
        };
        Ok(self.inner.to_ixdtf_string(display))
    }

    pub fn to_json(&self) -> String {
        self.inner.to_ixdtf_string(DisplayCalendar::Auto)
    }

    pub fn value_of(&self, ctx: Ctx<'_>) -> Result<()> {
        Err(throw_value_of(&ctx, "Temporal.PlainDate"))
    }

    #[qjs(prop, rename = PredefinedAtom::SymbolToStringTag, configurable)]
    pub fn to_string_tag() -> &'static str {
        "Temporal.PlainDate"
    }
}
