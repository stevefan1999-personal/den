use rquickjs::{Ctx, JsLifetime, Result, Value, atom::PredefinedAtom, class::Trace, prelude::Opt};
use temporal_rs::options::DisplayCalendar;

use crate::convert::{
    bag_overflow, bag_value, optional_calendar, options_bag, ordering_i32, required_i32,
    required_u8, throw_value_of, to_difference_settings, to_display_calendar, to_duration,
    to_plain_year_month, unwrap_temporal,
};
use crate::duration::Duration;

#[derive(Trace, JsLifetime, Clone)]
#[rquickjs::class(rename = "PlainYearMonth", frozen)]
pub struct PlainYearMonth {
    #[qjs(skip_trace)]
    pub(crate) inner: temporal_rs::PlainYearMonth,
}

impl PlainYearMonth {
    pub(crate) fn wrap(inner: temporal_rs::PlainYearMonth) -> Self {
        Self { inner }
    }
}

#[rquickjs::methods(rename_all = "camelCase")]
impl PlainYearMonth {
    #[qjs(constructor)]
    pub fn new<'js>(
        iso_year: Opt<Value<'js>>,
        iso_month: Opt<Value<'js>>,
        calendar: Opt<Value<'js>>,
        reference_iso_day: Opt<Value<'js>>,
        ctx: Ctx<'js>,
    ) -> Result<Self> {
        let year = required_i32(&ctx, iso_year, "year")?;
        let month = required_u8(&ctx, iso_month, "month")?;
        let calendar = optional_calendar(&ctx, calendar)?;
        let reference_day = match reference_iso_day.0 {
            None => None,
            Some(value) if value.is_undefined() => None,
            Some(value) => {
                let integer = crate::convert::to_integer_with_truncation(&ctx, &value)?;
                Some(u8::try_from(integer).map_err(|_| {
                    rquickjs::Exception::throw_range(&ctx, "integer is out of range")
                })?)
            }
        };
        unwrap_temporal(
            &ctx,
            temporal_rs::PlainYearMonth::try_new(year, month, reference_day, calendar),
        )
        .map(Self::wrap)
    }

    #[qjs(static)]
    pub fn from<'js>(item: Value<'js>, ctx: Ctx<'js>) -> Result<Self> {
        to_plain_year_month(&ctx, &item).map(Self::wrap)
    }

    #[qjs(static)]
    pub fn compare<'js>(one: Value<'js>, two: Value<'js>, ctx: Ctx<'js>) -> Result<i32> {
        let left = to_plain_year_month(&ctx, &one)?;
        let right = to_plain_year_month(&ctx, &two)?;
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
    pub fn days_in_year(&self) -> u16 {
        self.inner.days_in_year()
    }

    #[qjs(get)]
    pub fn days_in_month(&self) -> u16 {
        self.inner.days_in_month()
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
        let overflow = bag_overflow(&ctx, &options_bag(&ctx, options)?)?.unwrap_or_default();
        unwrap_temporal(&ctx, self.inner.add(&duration, overflow)).map(Self::wrap)
    }

    pub fn subtract<'js>(
        &self,
        duration_like: Value<'js>,
        options: Opt<Value<'js>>,
        ctx: Ctx<'js>,
    ) -> Result<Self> {
        let duration = to_duration(&ctx, &duration_like)?;
        let overflow = bag_overflow(&ctx, &options_bag(&ctx, options)?)?.unwrap_or_default();
        unwrap_temporal(&ctx, self.inner.subtract(&duration, overflow)).map(Self::wrap)
    }

    pub fn until<'js>(
        &self,
        other: Value<'js>,
        options: Opt<Value<'js>>,
        ctx: Ctx<'js>,
    ) -> Result<Duration> {
        let other = to_plain_year_month(&ctx, &other)?;
        let settings = to_difference_settings(&ctx, &options_bag(&ctx, options)?)?;
        unwrap_temporal(&ctx, self.inner.until(&other, settings)).map(Duration::wrap)
    }

    pub fn since<'js>(
        &self,
        other: Value<'js>,
        options: Opt<Value<'js>>,
        ctx: Ctx<'js>,
    ) -> Result<Duration> {
        let other = to_plain_year_month(&ctx, &other)?;
        let settings = to_difference_settings(&ctx, &options_bag(&ctx, options)?)?;
        unwrap_temporal(&ctx, self.inner.since(&other, settings)).map(Duration::wrap)
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
        Err(throw_value_of(&ctx, "Temporal.PlainYearMonth"))
    }

    #[qjs(prop, rename = PredefinedAtom::SymbolToStringTag, configurable)]
    pub fn to_string_tag() -> &'static str {
        "Temporal.PlainYearMonth"
    }
}
