use std::str::FromStr as _;

use rquickjs::{
    Ctx, Exception, JsLifetime, Object, Result, Value,
    atom::PredefinedAtom,
    class::Trace,
    prelude::{Opt, Rest},
};
use temporal_rs::{
    Calendar, MonthCode,
    fields::{CalendarFields, DateTimeFields},
    options::{DisplayCalendar, ToStringRoundingOptions},
    partial::{PartialDateTime, PartialTime},
};

use crate::{
    convert::{
        calendar_slot, ctor_required_i32, ctor_required_u8, difference_settings, get_defined,
        optional_enum, optional_month_code, optional_truncated_i32, optional_truncated_u8,
        optional_truncated_u16, options_object, overflow_option, probe_class,
        reject_calendar_or_time_zone, require_object, rounding_options, throw_value_of,
        to_duration, to_string_rounding, to_time_zone, truncated_u8_or_zero, truncated_u16_or_zero,
        unwrap_temporal,
    },
    duration::Duration,
    plain_date::PlainDate,
    plain_time::{PlainTime, to_temporal_time},
    zoned_date_time::ZonedDateTime,
};

#[derive(Trace, JsLifetime, Clone)]
#[rquickjs::class(rename = "PlainDateTime", frozen)]
pub struct PlainDateTime {
    #[qjs(skip_trace)]
    pub(crate) inner: temporal_rs::PlainDateTime,
}

impl PlainDateTime {
    pub(crate) const fn wrap(inner: temporal_rs::PlainDateTime) -> Self { Self { inner } }
}

fn has_calendar_or_time_zone_slot<'js>(ctx: &Ctx<'js>, value: &Value<'js>) -> bool {
    calendar_slot(ctx, value).is_some() || probe_class::<PlainTime>(ctx, value).is_some()
}

/// `ParseTemporalCalendarString` + canonicalize (bags, `withCalendar`).
fn to_calendar_like<'js>(ctx: &Ctx<'js>, value: &Value<'js>) -> Result<Calendar> {
    if let Some(calendar) = calendar_slot(ctx, value) {
        return Ok(calendar);
    }
    if !value.is_string() {
        return Err(Exception::throw_type(
            ctx,
            "calendar must be a calendar identifier string",
        ));
    }
    let identifier = value.get::<String>()?;
    unwrap_temporal(ctx, Calendar::from_str(&identifier))
}

/// Constructor calendar argument: identifier only, not an ISO date/time string.
fn to_constructor_calendar<'js>(ctx: &Ctx<'js>, value: &Value<'js>) -> Result<Calendar> {
    if let Some(calendar) = calendar_slot(ctx, value) {
        return Ok(calendar);
    }
    if !value.is_string() {
        return Err(Exception::throw_type(
            ctx,
            "calendar must be a calendar identifier string",
        ));
    }
    let identifier = value.get::<String>()?;
    unwrap_temporal(ctx, Calendar::try_from_utf8(identifier.as_bytes()))
}

fn get_calendar_with_iso_default<'js>(ctx: &Ctx<'js>, object: &Object<'js>) -> Result<Calendar> {
    get_defined(object, "calendar")?
        .map_or(Ok(Calendar::ISO), |value| to_calendar_like(ctx, &value))
}

/// PrepareTemporalFields Get order: day, hour, micro, milli, minute, month,
/// monthCode, nano, second, year.
fn datetime_fields_from_object<'js>(
    ctx: &Ctx<'js>, object: &Object<'js>,
) -> Result<(DateTimeFields, Option<String>)> {
    let day = optional_truncated_u8(ctx, object, "day")?;
    let hour = optional_truncated_u8(ctx, object, "hour")?;
    let microsecond = optional_truncated_u16(ctx, object, "microsecond")?;
    let millisecond = optional_truncated_u16(ctx, object, "millisecond")?;
    let minute = optional_truncated_u8(ctx, object, "minute")?;
    let month = optional_truncated_u8(ctx, object, "month")?;
    let month_code_text = optional_month_code(ctx, object)?;
    let nanosecond = optional_truncated_u16(ctx, object, "nanosecond")?;
    let second = optional_truncated_u8(ctx, object, "second")?;
    let year = optional_truncated_i32(ctx, object, "year")?;
    let mut calendar_fields = CalendarFields::new();
    if let Some(year) = year {
        calendar_fields = calendar_fields.with_year(year);
    }
    if let Some(month) = month {
        calendar_fields = calendar_fields.with_month(month);
    }
    if let Some(day) = day {
        calendar_fields = calendar_fields.with_day(day);
    }
    Ok((
        DateTimeFields {
            calendar_fields,
            time: PartialTime {
                hour,
                minute,
                second,
                millisecond,
                microsecond,
                nanosecond,
            },
        },
        month_code_text,
    ))
}

fn apply_month_code(
    ctx: &Ctx<'_>, mut fields: DateTimeFields, month_code: Option<String>,
) -> Result<DateTimeFields> {
    if let Some(code) = month_code {
        let month_code = unwrap_temporal(ctx, MonthCode::try_from_utf8(code.as_bytes()))?;
        fields.calendar_fields = fields.calendar_fields.with_month_code(month_code);
    }
    Ok(fields)
}

fn to_pdt<'js>(
    ctx: &Ctx<'js>, value: &Value<'js>, options: Opt<Value<'js>>,
) -> Result<temporal_rs::PlainDateTime> {
    if let Some(date_time) = probe_class::<PlainDateTime>(ctx, value) {
        let _overflow = overflow_option(ctx, options)?;
        return Ok(date_time.inner);
    }
    if let Some(zoned) = probe_class::<ZonedDateTime>(ctx, value) {
        let _overflow = overflow_option(ctx, options)?;
        return Ok(zoned.inner.to_plain_date_time());
    }
    if let Some(date) = probe_class::<PlainDate>(ctx, value) {
        let _overflow = overflow_option(ctx, options)?;
        return unwrap_temporal(ctx, date.inner.to_plain_date_time(None));
    }
    if value.is_string() {
        let string = value.get::<String>()?;
        let parsed = unwrap_temporal(
            ctx,
            temporal_rs::PlainDateTime::from_utf8(string.as_bytes()),
        )?;
        let _overflow = overflow_option(ctx, options)?;
        return Ok(parsed);
    }
    let object = require_object(ctx, value, "cannot convert value to Temporal.PlainDateTime")?;
    let calendar = get_calendar_with_iso_default(ctx, &object)?;
    let (fields, month_code) = datetime_fields_from_object(ctx, &object)?;
    let overflow = overflow_option(ctx, options)?;
    let fields = apply_month_code(ctx, fields, month_code)?;
    unwrap_temporal(
        ctx,
        temporal_rs::PlainDateTime::from_partial(PartialDateTime { fields, calendar }, overflow),
    )
}

#[rquickjs::methods(rename_all = "camelCase")]
impl PlainDateTime {
    #[qjs(constructor)]
    pub fn new<'js>(
        iso_year: Opt<Value<'js>>, iso_month: Opt<Value<'js>>, iso_day: Opt<Value<'js>>,
        rest: Rest<Value<'js>>, ctx: Ctx<'js>,
    ) -> Result<Self> {
        let year = ctor_required_i32(&ctx, iso_year)?;
        let month = ctor_required_u8(&ctx, iso_month)?;
        let day = ctor_required_u8(&ctx, iso_day)?;
        let mut rest = rest.0.into_iter();
        let hour = truncated_u8_or_zero(&ctx, Opt(rest.next()))?;
        let minute = truncated_u8_or_zero(&ctx, Opt(rest.next()))?;
        let second = truncated_u8_or_zero(&ctx, Opt(rest.next()))?;
        let millisecond = truncated_u16_or_zero(&ctx, Opt(rest.next()))?;
        let microsecond = truncated_u16_or_zero(&ctx, Opt(rest.next()))?;
        let nanosecond = truncated_u16_or_zero(&ctx, Opt(rest.next()))?;
        let calendar = match rest.next() {
            Some(value) if !value.is_undefined() => to_constructor_calendar(&ctx, &value)?,
            _ => Calendar::ISO,
        };
        unwrap_temporal(
            &ctx,
            temporal_rs::PlainDateTime::try_new(
                year,
                month,
                day,
                hour,
                minute,
                second,
                millisecond,
                microsecond,
                nanosecond,
                calendar,
            ),
        )
        .map(Self::wrap)
    }

    #[qjs(static)]
    pub fn from<'js>(item: Value<'js>, options: Opt<Value<'js>>, ctx: Ctx<'js>) -> Result<Self> {
        to_pdt(&ctx, &item, options).map(Self::wrap)
    }

    #[qjs(static)]
    pub fn compare<'js>(one: Value<'js>, two: Value<'js>, ctx: Ctx<'js>) -> Result<i32> {
        let left = to_pdt(&ctx, &one, Opt(None))?;
        let right = to_pdt(&ctx, &two, Opt(None))?;
        Ok(left.compare_iso(&right) as i32)
    }

    #[qjs(get, configurable)]
    pub fn calendar_id(&self) -> &'static str { self.inner.calendar().identifier() }

    #[qjs(get, configurable)]
    pub fn year(&self) -> i32 { self.inner.year() }

    #[qjs(get, configurable)]
    pub fn month(&self) -> u8 { self.inner.month() }

    #[qjs(get, configurable)]
    pub fn month_code(&self) -> String { self.inner.month_code().as_str().to_string() }

    #[qjs(get, configurable)]
    pub fn day(&self) -> u8 { self.inner.day() }

    #[qjs(get, configurable)]
    pub fn hour(&self) -> u8 { self.inner.hour() }

    #[qjs(get, configurable)]
    pub fn minute(&self) -> u8 { self.inner.minute() }

    #[qjs(get, configurable)]
    pub fn second(&self) -> u8 { self.inner.second() }

    #[qjs(get, configurable)]
    pub fn millisecond(&self) -> u16 { self.inner.millisecond() }

    #[qjs(get, configurable)]
    pub fn microsecond(&self) -> u16 { self.inner.microsecond() }

    #[qjs(get, configurable)]
    pub fn nanosecond(&self) -> u16 { self.inner.nanosecond() }

    #[qjs(get, configurable)]
    pub fn day_of_week(&self) -> u16 { self.inner.day_of_week() }

    #[qjs(get, configurable)]
    pub fn day_of_year(&self) -> u16 { self.inner.day_of_year() }

    #[qjs(get, configurable)]
    pub fn week_of_year(&self) -> Option<u8> { self.inner.week_of_year() }

    #[qjs(get, configurable)]
    pub fn year_of_week(&self) -> Option<i32> { self.inner.year_of_week() }

    #[qjs(get, configurable)]
    pub fn days_in_week(&self) -> u16 { self.inner.days_in_week() }

    #[qjs(get, configurable)]
    pub fn days_in_month(&self) -> u16 { self.inner.days_in_month() }

    #[qjs(get, configurable)]
    pub fn days_in_year(&self) -> u16 { self.inner.days_in_year() }

    #[qjs(get, configurable)]
    pub fn months_in_year(&self) -> u16 { self.inner.months_in_year() }

    #[qjs(get, configurable)]
    pub fn in_leap_year(&self) -> bool { self.inner.in_leap_year() }

    #[qjs(get, configurable)]
    pub fn era(&self) -> Option<String> { self.inner.era().map(|era| era.to_string()) }

    #[qjs(get, configurable)]
    pub fn era_year(&self) -> Option<i32> { self.inner.era_year() }

    pub fn add<'js>(
        &self, duration_like: Value<'js>, options: Opt<Value<'js>>, ctx: Ctx<'js>,
    ) -> Result<Self> {
        let duration = to_duration(&ctx, &duration_like)?;
        let overflow = overflow_option(&ctx, options)?;
        unwrap_temporal(&ctx, self.inner.add(&duration, overflow)).map(Self::wrap)
    }

    pub fn subtract<'js>(
        &self, duration_like: Value<'js>, options: Opt<Value<'js>>, ctx: Ctx<'js>,
    ) -> Result<Self> {
        let duration = to_duration(&ctx, &duration_like)?;
        let overflow = overflow_option(&ctx, options)?;
        unwrap_temporal(&ctx, self.inner.subtract(&duration, overflow)).map(Self::wrap)
    }

    pub fn until<'js>(
        &self, other: Value<'js>, options: Opt<Value<'js>>, ctx: Ctx<'js>,
    ) -> Result<Duration> {
        let other = to_pdt(&ctx, &other, Opt(None))?;
        let settings = difference_settings(&ctx, options)?;
        unwrap_temporal(&ctx, self.inner.until(&other, settings)).map(Duration::wrap)
    }

    pub fn since<'js>(
        &self, other: Value<'js>, options: Opt<Value<'js>>, ctx: Ctx<'js>,
    ) -> Result<Duration> {
        let other = to_pdt(&ctx, &other, Opt(None))?;
        let settings = difference_settings(&ctx, options)?;
        unwrap_temporal(&ctx, self.inner.since(&other, settings)).map(Duration::wrap)
    }

    pub fn equals<'js>(&self, other: Value<'js>, ctx: Ctx<'js>) -> Result<bool> {
        Ok(self.inner == to_pdt(&ctx, &other, Opt(None))?)
    }

    pub fn with<'js>(
        &self, item: Value<'js>, options: Opt<Value<'js>>, ctx: Ctx<'js>,
    ) -> Result<Self> {
        if has_calendar_or_time_zone_slot(&ctx, &item) {
            return Err(Exception::throw_type(
                &ctx,
                "calendar is not allowed in with()",
            ));
        }
        let object = require_object(&ctx, &item, "argument must be an object")?;
        reject_calendar_or_time_zone(
            &ctx,
            &object,
            "calendar is not allowed in with()",
            "timeZone is not allowed in with()",
        )?;
        let (fields, month_code) = datetime_fields_from_object(&ctx, &object)?;
        let overflow = overflow_option(&ctx, options)?;
        let fields = apply_month_code(&ctx, fields, month_code)?;
        unwrap_temporal(&ctx, self.inner.with(fields, overflow)).map(Self::wrap)
    }

    pub fn with_calendar<'js>(&self, calendar: Value<'js>, ctx: Ctx<'js>) -> Result<Self> {
        if calendar.is_undefined() {
            return Err(Exception::throw_type(&ctx, "calendar is required"));
        }
        Ok(Self::wrap(
            self.inner.with_calendar(to_calendar_like(&ctx, &calendar)?),
        ))
    }

    pub fn with_plain_time<'js>(&self, time: Opt<Value<'js>>, ctx: Ctx<'js>) -> Result<Self> {
        let time = match time.0 {
            Some(value) if !value.is_undefined() => Some(to_temporal_time(&ctx, &value)?),
            _ => None,
        };
        unwrap_temporal(&ctx, self.inner.with_time(time)).map(Self::wrap)
    }

    pub fn round<'js>(&self, options: Value<'js>, ctx: Ctx<'js>) -> Result<Self> {
        let rounding = rounding_options(&ctx, &options)?;
        unwrap_temporal(&ctx, self.inner.round(rounding)).map(Self::wrap)
    }

    pub fn to_plain_date(&self) -> PlainDate { PlainDate::wrap(self.inner.to_plain_date()) }

    pub fn to_plain_time(&self) -> PlainTime { PlainTime::wrap(self.inner.to_plain_time()) }

    pub fn to_zoned_date_time<'js>(
        &self, time_zone: Value<'js>, options: Opt<Value<'js>>, ctx: Ctx<'js>,
    ) -> Result<ZonedDateTime> {
        let zone = to_time_zone(&ctx, &time_zone)?;
        let disambiguation = options_object(&ctx, options)?
            .map_or(Ok(None), |object| {
                optional_enum(&ctx, &object, "disambiguation", "disambiguation")
            })?
            .unwrap_or_default();
        unwrap_temporal(&ctx, self.inner.to_zoned_date_time(zone, disambiguation))
            .map(ZonedDateTime::wrap)
    }

    pub fn to_string<'js>(&self, options: Opt<Value<'js>>, ctx: Ctx<'js>) -> Result<String> {
        let object = options_object(&ctx, options)?;
        let (rounding, display) = match object {
            None => (ToStringRoundingOptions::default(), DisplayCalendar::Auto),
            Some(object) => {
                let display = optional_enum(&ctx, &object, "calendarName", "calendarName option")?
                    .unwrap_or(DisplayCalendar::Auto);
                let rounding =
                    to_string_rounding(&ctx, &object, "fractionalSecondDigits must be finite")?;
                (rounding, display)
            }
        };
        unwrap_temporal(&ctx, self.inner.to_ixdtf_string(rounding, display))
    }

    #[qjs(rename = "toJSON")]
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
    pub const fn to_string_tag() -> &'static str { "Temporal.PlainDateTime" }
}
