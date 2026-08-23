use rquickjs::{Ctx, JsLifetime, Result, Value, atom::PredefinedAtom, class::Trace, prelude::Opt};
use temporal_rs::options::{DisplayCalendar, Overflow};

use crate::convert::{
    bag_value, optional_calendar, options_bag, required_u8, throw_value_of, to_display_calendar,
    to_integer_with_truncation, to_plain_month_day, unwrap_temporal,
};

#[derive(Trace, JsLifetime, Clone)]
#[rquickjs::class(rename = "PlainMonthDay", frozen)]
pub struct PlainMonthDay {
    #[qjs(skip_trace)]
    pub(crate) inner: temporal_rs::PlainMonthDay,
}

impl PlainMonthDay {
    pub(crate) fn wrap(inner: temporal_rs::PlainMonthDay) -> Self {
        Self { inner }
    }
}

#[rquickjs::methods(rename_all = "camelCase")]
impl PlainMonthDay {
    #[qjs(constructor)]
    pub fn new<'js>(
        iso_month: Opt<Value<'js>>,
        iso_day: Opt<Value<'js>>,
        calendar: Opt<Value<'js>>,
        reference_iso_year: Opt<Value<'js>>,
        ctx: Ctx<'js>,
    ) -> Result<Self> {
        let month = required_u8(&ctx, iso_month, "month")?;
        let day = required_u8(&ctx, iso_day, "day")?;
        let calendar = optional_calendar(&ctx, calendar)?;
        let reference_year = match reference_iso_year.0 {
            None => None,
            Some(value) if value.is_undefined() => None,
            Some(value) => {
                let integer = to_integer_with_truncation(&ctx, &value)?;
                Some(i32::try_from(integer).map_err(|_| {
                    rquickjs::Exception::throw_range(&ctx, "integer is out of range")
                })?)
            }
        };
        unwrap_temporal(
            &ctx,
            temporal_rs::PlainMonthDay::new_with_overflow(
                month,
                day,
                calendar,
                Overflow::Reject,
                reference_year,
            ),
        )
        .map(Self::wrap)
    }

    #[qjs(static)]
    pub fn from<'js>(item: Value<'js>, ctx: Ctx<'js>) -> Result<Self> {
        to_plain_month_day(&ctx, &item).map(Self::wrap)
    }

    #[qjs(get)]
    pub fn calendar_id(&self) -> &'static str {
        self.inner.calendar_id()
    }

    #[qjs(get)]
    pub fn month_code(&self) -> String {
        self.inner.month_code().as_str().to_string()
    }

    #[qjs(get)]
    pub fn day(&self) -> u8 {
        self.inner.day()
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
        Err(throw_value_of(&ctx, "Temporal.PlainMonthDay"))
    }

    #[qjs(prop, rename = PredefinedAtom::SymbolToStringTag, configurable)]
    pub fn to_string_tag() -> &'static str {
        "Temporal.PlainMonthDay"
    }
}
