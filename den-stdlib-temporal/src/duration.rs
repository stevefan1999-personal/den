use std::str::FromStr as _;

use rquickjs::{
    Ctx, Exception, FromJs, Function, JsLifetime, Object, Result, Value, atom::PredefinedAtom,
    class::Trace, function::This, prelude::Opt,
};
use temporal_rs::{
    Calendar, MonthCode, UtcOffset,
    fields::{CalendarFields, ZonedDateTimeFields},
    options::{
        Disambiguation, OffsetDisambiguation, Overflow, RelativeTo, RoundingIncrement,
        RoundingMode, RoundingOptions, ToStringRoundingOptions, Unit,
    },
    parsers::Precision,
    partial::{PartialDate, PartialDuration, PartialTime, PartialZonedDateTime},
};

use crate::{
    convert::{
        ctor_integer_if_integral, ctor_integer_if_integral_i128, js_to_string, ordering_i32,
        probe_class, throw_value_of, to_calendar, to_integer_if_integral,
        to_integer_if_integral_i64, to_integer_with_truncation, to_number, to_time_zone, to_unit,
        unwrap_temporal,
    },
    plain_date::PlainDate,
    plain_date_time::PlainDateTime,
    plain_month_day::PlainMonthDay,
    plain_year_month::PlainYearMonth,
    zoned_date_time::ZonedDateTime,
};

#[derive(Trace, JsLifetime, Clone, Copy)]
#[rquickjs::class(rename = "Duration", frozen)]
pub struct Duration {
    #[qjs(skip_trace)]
    pub(crate) inner: temporal_rs::Duration,
}

impl Duration {
    pub(crate) fn wrap(inner: temporal_rs::Duration) -> Self {
        Self {
            inner: temporal_rs::Duration::new(
                snap_i64(inner.years()),
                snap_i64(inner.months()),
                snap_i64(inner.weeks()),
                snap_i64(inner.days()),
                snap_i64(inner.hours()),
                snap_i64(inner.minutes()),
                snap_i64(inner.seconds()),
                snap_i64(inner.milliseconds()),
                snap_i128(inner.microseconds()),
                snap_i128(inner.nanoseconds()),
            )
            .unwrap_or(inner),
        }
    }
}

const fn snap_i64(value: i64) -> i64 { value as f64 as i64 }

const fn snap_i128(value: i128) -> i128 { value as f64 as i128 }

fn defined_property<'js>(object: &Object<'js>, key: &str) -> Result<Option<Value<'js>>> {
    let value = object.get::<_, Value>(key)?;
    if value.is_undefined() {
        Ok(None)
    } else {
        Ok(Some(value))
    }
}

fn options_object<'js>(ctx: &Ctx<'js>, options: Opt<Value<'js>>) -> Result<Object<'js>> {
    match options.0 {
        None => Object::new(ctx.clone()),
        Some(value) if value.is_undefined() => Object::new(ctx.clone()),
        Some(value) => value
            .as_object()
            .cloned()
            .ok_or_else(|| Exception::throw_type(ctx, "options must be an object")),
    }
}

fn required_options_object<'js>(ctx: &Ctx<'js>, options: &Value<'js>) -> Result<Object<'js>> {
    if options.is_undefined() || options.is_null() {
        return Err(Exception::throw_type(ctx, "options are required"));
    }
    if let Some(object) = options.as_object() {
        return Ok(object.clone());
    }
    // GetOptionsObject is ToObject. A function is an object; rquickjs
    // `as_object()` is None for callables.
    if options.is_function() {
        return Function::from_js(ctx, options.clone()).map(Function::into_inner);
    }
    Err(Exception::throw_type(ctx, "options must be an object"))
}

fn out_of_range<'js>(ctx: &Ctx<'js>, error: impl core::fmt::Debug) -> rquickjs::Error {
    drop(error);
    Exception::throw_range(ctx, "integer is out of range")
}

fn string_option<'js>(ctx: &Ctx<'js>, value: &Value<'js>) -> Result<String> {
    if value.is_symbol() {
        return Err(Exception::throw_type(
            ctx,
            "cannot convert a Symbol to a String",
        ));
    }
    let Some(object) = value.as_object() else {
        return js_to_string(ctx, value);
    };
    let to_string: Function = object.get("toString")?;
    let primitive: Value = to_string.call((This(object.clone()),))?;
    if primitive.is_symbol() {
        return Err(Exception::throw_type(
            ctx,
            "cannot convert a Symbol to a String",
        ));
    }
    if primitive.is_object() {
        return Err(Exception::throw_type(
            ctx,
            "cannot convert object to a primitive string",
        ));
    }
    js_to_string(ctx, &primitive)
}

fn truncated_i32<'js>(ctx: &Ctx<'js>, value: &Value<'js>) -> Result<i32> {
    let integer = to_integer_with_truncation(ctx, value)?;
    i32::try_from(integer).map_err(|error| out_of_range(ctx, error))
}

fn optional_integral_i64<'js>(
    ctx: &Ctx<'js>, object: &Object<'js>, key: &str,
) -> Result<Option<i64>> {
    defined_property(object, key)?
        .map_or(Ok(None), |value| to_integer_if_integral_i64(ctx, &value).map(Some))
}

fn optional_integral_i128<'js>(
    ctx: &Ctx<'js>, object: &Object<'js>, key: &str,
) -> Result<Option<i128>> {
    defined_property(object, key)?
        .map_or(Ok(None), |value| to_integer_if_integral(ctx, &value).map(Some))
}

fn optional_truncated_u8<'js>(
    ctx: &Ctx<'js>, object: &Object<'js>, key: &str,
) -> Result<Option<u8>> {
    defined_property(object, key)?.map_or(Ok(None), |value| {
        u8::try_from(truncated_i32(ctx, &value)?).map(Some).map_err(|error| out_of_range(ctx, error))
    })
}

fn optional_truncated_u16<'js>(
    ctx: &Ctx<'js>, object: &Object<'js>, key: &str,
) -> Result<Option<u16>> {
    defined_property(object, key)?.map_or(Ok(None), |value| {
        u16::try_from(truncated_i32(ctx, &value)?).map(Some).map_err(|error| out_of_range(ctx, error))
    })
}

fn partial_duration_from_object<'js>(
    ctx: &Ctx<'js>, object: &Object<'js>,
) -> Result<PartialDuration> {
    let days = optional_integral_i64(ctx, object, "days")?;
    let hours = optional_integral_i64(ctx, object, "hours")?;
    let microseconds = optional_integral_i128(ctx, object, "microseconds")?;
    let milliseconds = optional_integral_i64(ctx, object, "milliseconds")?;
    let minutes = optional_integral_i64(ctx, object, "minutes")?;
    let months = optional_integral_i64(ctx, object, "months")?;
    let nanoseconds = optional_integral_i128(ctx, object, "nanoseconds")?;
    let seconds = optional_integral_i64(ctx, object, "seconds")?;
    let weeks = optional_integral_i64(ctx, object, "weeks")?;
    let years = optional_integral_i64(ctx, object, "years")?;
    let partial = PartialDuration {
        years,
        months,
        weeks,
        days,
        hours,
        minutes,
        seconds,
        milliseconds,
        microseconds,
        nanoseconds,
    };
    if partial.is_empty() {
        return Err(Exception::throw_type(
            ctx,
            "Temporal.Duration requires at least one field",
        ));
    }
    Ok(partial)
}

fn to_temporal_duration<'js>(
    ctx: &Ctx<'js>, value: &Value<'js>,
) -> Result<temporal_rs::Duration> {
    if let Some(duration) = probe_class::<Duration>(ctx, value) {
        return Ok(duration.inner);
    }
    if value.is_string() {
        let string = value.get::<String>()?;
        return unwrap_temporal(ctx, temporal_rs::Duration::from_utf8(string.as_bytes()));
    }
    let Some(object) = value.as_object() else {
        return Err(Exception::throw_type(
            ctx,
            "cannot convert value to Temporal.Duration",
        ));
    };
    let partial = partial_duration_from_object(ctx, object)?;
    unwrap_temporal(ctx, temporal_rs::Duration::from_partial_duration(partial))
}

fn relative_to_option<'js>(
    ctx: &Ctx<'js>, object: &Object<'js>,
) -> Result<Option<RelativeTo>> {
    let Some(value) = defined_property(object, "relativeTo")? else {
        return Ok(None);
    };
    if let Some(zoned) = probe_class::<ZonedDateTime>(ctx, &value) {
        return Ok(Some(RelativeTo::ZonedDateTime(zoned.inner)));
    }
    if let Some(date) = probe_class::<PlainDate>(ctx, &value) {
        return Ok(Some(RelativeTo::PlainDate(date.inner)));
    }
    if let Some(date_time) = probe_class::<PlainDateTime>(ctx, &value) {
        return Ok(Some(RelativeTo::PlainDate(date_time.inner.to_plain_date())));
    }
    if value.is_string() {
        let string = value.get::<String>()?;
        return unwrap_temporal(ctx, RelativeTo::try_from_str(&string)).map(Some);
    }
    let Some(bag) = value.as_object() else {
        return Err(Exception::throw_type(
            ctx,
            "relativeTo must be a Temporal date",
        ));
    };
    let calendar = defined_property(bag, "calendar")?.map_or(Ok(Calendar::ISO), |calendar| {
        if let Some(date) = probe_class::<PlainDate>(ctx, &calendar) {
            return Ok(date.inner.calendar().clone());
        }
        if let Some(date_time) = probe_class::<PlainDateTime>(ctx, &calendar) {
            return Ok(date_time.inner.calendar().clone());
        }
        if let Some(year_month) = probe_class::<PlainYearMonth>(ctx, &calendar) {
            return Ok(year_month.inner.calendar().clone());
        }
        if let Some(month_day) = probe_class::<PlainMonthDay>(ctx, &calendar) {
            return Ok(month_day.inner.calendar().clone());
        }
        if let Some(zoned) = probe_class::<ZonedDateTime>(ctx, &calendar) {
            return Ok(zoned.inner.calendar().clone());
        }
        to_calendar(ctx, &calendar)
    })?;
    let day = optional_truncated_u8(ctx, bag, "day")?;
    let hour = optional_truncated_u8(ctx, bag, "hour")?;
    let microsecond = optional_truncated_u16(ctx, bag, "microsecond")?;
    let millisecond = optional_truncated_u16(ctx, bag, "millisecond")?;
    let minute = optional_truncated_u8(ctx, bag, "minute")?;
    let month = optional_truncated_u8(ctx, bag, "month")?;
    let month_code = defined_property(bag, "monthCode")?.map_or(Ok(None), |code| {
        let text = js_to_string(ctx, &code)?;
        unwrap_temporal(ctx, MonthCode::try_from_utf8(text.as_bytes())).map(Some)
    })?;
    let nanosecond = optional_truncated_u16(ctx, bag, "nanosecond")?;
    let utc_offset = defined_property(bag, "offset")?.map_or(Ok(None), |offset| {
        let text = if offset.is_string() {
            offset.get::<String>()?
        } else if offset.is_object() {
            string_option(ctx, &offset)?
        } else {
            return Err(Exception::throw_type(ctx, "offset must be a string"));
        };
        UtcOffset::from_str(&text).map(Some).map_err(|error| out_of_range(ctx, error))
    })?;
    let second = optional_truncated_u8(ctx, bag, "second")?;
    let zone = defined_property(bag, "timeZone")?
        .map_or(Ok(None), |time_zone| to_time_zone(ctx, &time_zone).map(Some))?;
    let year = defined_property(bag, "year")?
        .map_or(Ok(None), |value| truncated_i32(ctx, &value).map(Some))?;
    let date_fields = CalendarFields::new()
        .with_optional_year(year)
        .with_optional_month(month)
        .with_optional_month_code(month_code)
        .with_optional_day(day);
    let clock = PartialTime {
        hour,
        minute,
        second,
        millisecond,
        microsecond,
        nanosecond,
    };
    match zone {
        None => unwrap_temporal(
            ctx,
            temporal_rs::PlainDate::from_partial(
                PartialDate { calendar_fields: date_fields, calendar },
                Some(Overflow::Constrain),
            ),
        )
        .map(|date| Some(RelativeTo::PlainDate(date))),
        Some(zone) => unwrap_temporal(
            ctx,
            temporal_rs::ZonedDateTime::from_partial(
                PartialZonedDateTime {
                    fields: ZonedDateTimeFields {
                        calendar_fields: date_fields,
                        time:            clock,
                        offset:          utc_offset,
                    },
                    timezone: Some(zone),
                    calendar,
                },
                Some(Overflow::Constrain),
                Some(Disambiguation::Compatible),
                Some(OffsetDisambiguation::Reject),
            ),
        )
        .map(|zoned| Some(RelativeTo::ZonedDateTime(zoned))),
    }
}

fn optional_unit<'js>(ctx: &Ctx<'js>, object: &Object<'js>, key: &str) -> Result<Option<Unit>> {
    defined_property(object, key)?.map_or(Ok(None), |value| {
        let name = string_option(ctx, &value)?;
        Unit::from_str(&name).map(Some).map_err(|error| out_of_range(ctx, error))
    })
}

fn rounding_mode_option<'js>(
    ctx: &Ctx<'js>, object: &Object<'js>,
) -> Result<Option<RoundingMode>> {
    defined_property(object, "roundingMode")?.map_or(Ok(None), |value| {
        let name = string_option(ctx, &value)?;
        unwrap_temporal(ctx, RoundingMode::from_str(&name)).map(Some)
    })
}

#[rquickjs::methods(rename_all = "camelCase")]
impl Duration {
    #[qjs(constructor)]
    pub fn new<'js>(
        years: Opt<Value<'js>>, months: Opt<Value<'js>>, weeks: Opt<Value<'js>>,
        days: Opt<Value<'js>>, hours: Opt<Value<'js>>, minutes: Opt<Value<'js>>,
        seconds: Opt<Value<'js>>, milliseconds: Opt<Value<'js>>, microseconds: Opt<Value<'js>>,
        nanoseconds: Opt<Value<'js>>, ctx: Ctx<'js>,
    ) -> Result<Self> {
        let inner = unwrap_temporal(
            &ctx,
            temporal_rs::Duration::new(
                ctor_integer_if_integral(&ctx, years)?,
                ctor_integer_if_integral(&ctx, months)?,
                ctor_integer_if_integral(&ctx, weeks)?,
                ctor_integer_if_integral(&ctx, days)?,
                ctor_integer_if_integral(&ctx, hours)?,
                ctor_integer_if_integral(&ctx, minutes)?,
                ctor_integer_if_integral(&ctx, seconds)?,
                ctor_integer_if_integral(&ctx, milliseconds)?,
                ctor_integer_if_integral_i128(&ctx, microseconds)?,
                ctor_integer_if_integral_i128(&ctx, nanoseconds)?,
            ),
        )?;
        Ok(Self::wrap(inner))
    }

    #[qjs(static)]
    pub fn from<'js>(item: Value<'js>, ctx: Ctx<'js>) -> Result<Self> {
        to_temporal_duration(&ctx, &item).map(Self::wrap)
    }

    #[qjs(static)]
    pub fn compare<'js>(
        one: Value<'js>, two: Value<'js>, options: Opt<Value<'js>>, ctx: Ctx<'js>,
    ) -> Result<i32> {
        let left = to_temporal_duration(&ctx, &one)?;
        let right = to_temporal_duration(&ctx, &two)?;
        let relative_to = relative_to_option(&ctx, &options_object(&ctx, options)?)?;
        unwrap_temporal(&ctx, left.compare(&right, relative_to)).map(ordering_i32)
    }

    #[qjs(get)]
    pub const fn years(&self) -> i64 { self.inner.years() }

    #[qjs(get)]
    pub const fn months(&self) -> i64 { self.inner.months() }

    #[qjs(get)]
    pub const fn weeks(&self) -> i64 { self.inner.weeks() }

    #[qjs(get)]
    pub const fn days(&self) -> i64 { self.inner.days() }

    #[qjs(get)]
    pub const fn hours(&self) -> i64 { self.inner.hours() }

    #[qjs(get)]
    pub const fn minutes(&self) -> i64 { self.inner.minutes() }

    #[qjs(get)]
    pub const fn seconds(&self) -> i64 { self.inner.seconds() }

    #[qjs(get)]
    pub const fn milliseconds(&self) -> i64 { self.inner.milliseconds() }

    #[qjs(get)]
    pub const fn microseconds(&self) -> f64 { self.inner.microseconds() as f64 }

    #[qjs(get)]
    pub const fn nanoseconds(&self) -> f64 { self.inner.nanoseconds() as f64 }

    #[qjs(get)]
    pub fn sign(&self) -> i8 { self.inner.sign() as i8 }

    #[qjs(get)]
    pub fn blank(&self) -> bool { self.inner.is_zero() }

    pub fn with<'js>(&self, temporal_duration_like: Value<'js>, ctx: Ctx<'js>) -> Result<Self> {
        let Some(object) = temporal_duration_like.as_object() else {
            return Err(Exception::throw_type(
                &ctx,
                "with() requires a duration-like object",
            ));
        };
        let partial = partial_duration_from_object(&ctx, object)?;
        unwrap_temporal(
            &ctx,
            temporal_rs::Duration::new(
                partial.years.unwrap_or_else(|| self.inner.years()),
                partial.months.unwrap_or_else(|| self.inner.months()),
                partial.weeks.unwrap_or_else(|| self.inner.weeks()),
                partial.days.unwrap_or_else(|| self.inner.days()),
                partial.hours.unwrap_or_else(|| self.inner.hours()),
                partial.minutes.unwrap_or_else(|| self.inner.minutes()),
                partial.seconds.unwrap_or_else(|| self.inner.seconds()),
                partial.milliseconds.unwrap_or_else(|| self.inner.milliseconds()),
                partial.microseconds.unwrap_or_else(|| self.inner.microseconds()),
                partial.nanoseconds.unwrap_or_else(|| self.inner.nanoseconds()),
            ),
        )
        .map(Self::wrap)
    }

    pub fn negated(&self) -> Self { Self::wrap(self.inner.negated()) }

    pub fn abs(&self) -> Self { Self::wrap(self.inner.abs()) }

    pub fn add<'js>(&self, other: Value<'js>, ctx: Ctx<'js>) -> Result<Self> {
        let other = to_temporal_duration(&ctx, &other)?;
        unwrap_temporal(&ctx, self.inner.add(&other)).map(Self::wrap)
    }

    pub fn subtract<'js>(&self, other: Value<'js>, ctx: Ctx<'js>) -> Result<Self> {
        let other = to_temporal_duration(&ctx, &other)?;
        unwrap_temporal(&ctx, self.inner.subtract(&other)).map(Self::wrap)
    }

    pub fn round<'js>(&self, options: Value<'js>, ctx: Ctx<'js>) -> Result<Self> {
        let (rounding, relative_to) = if options.is_string() {
            let mut rounding = RoundingOptions::default();
            rounding.smallest_unit = Some(to_unit(&ctx, &options)?);
            (rounding, None)
        } else {
            let object = required_options_object(&ctx, &options)?;
            let largest_unit = optional_unit(&ctx, &object, "largestUnit")?;
            let relative_to = relative_to_option(&ctx, &object)?;
            let increment = defined_property(&object, "roundingIncrement")?.map_or(
                Ok(None),
                |value| unwrap_temporal(&ctx, RoundingIncrement::try_from(to_number(&ctx, &value)?)).map(Some),
            )?;
            let rounding_mode = rounding_mode_option(&ctx, &object)?;
            let smallest_unit = optional_unit(&ctx, &object, "smallestUnit")?;
            if largest_unit.is_none() && smallest_unit.is_none() {
                return Err(Exception::throw_range(
                    &ctx,
                    "round requires largestUnit or smallestUnit",
                ));
            }
            let mut rounding = RoundingOptions::default();
            rounding.largest_unit = largest_unit.or(Some(Unit::Auto));
            rounding.smallest_unit = smallest_unit;
            rounding.rounding_mode = rounding_mode;
            rounding.increment = increment;
            (rounding, relative_to)
        };
        unwrap_temporal(&ctx, self.inner.round(rounding, relative_to)).map(Self::wrap)
    }

    pub fn total<'js>(&self, options: Value<'js>, ctx: Ctx<'js>) -> Result<f64> {
        let (unit, relative_to) = if options.is_string() {
            (to_unit(&ctx, &options)?, None)
        } else {
            let object = required_options_object(&ctx, &options)?;
            let relative_to = relative_to_option(&ctx, &object)?;
            let unit = optional_unit(&ctx, &object, "unit")?.ok_or_else(|| {
                Exception::throw_range(&ctx, "unit is required")
            })?;
            (unit, relative_to)
        };
        unwrap_temporal(&ctx, self.inner.total(unit, relative_to)).map(|total| total.as_inner())
    }

    pub fn to_string<'js>(&self, options: Opt<Value<'js>>, ctx: Ctx<'js>) -> Result<String> {
        let object = options_object(&ctx, options)?;
        let precision = match defined_property(&object, "fractionalSecondDigits")? {
            None => Precision::Auto,
            Some(value) if value.is_number() => {
                let number = to_number(&ctx, &value)?;
                if !number.is_finite() {
                    return Err(Exception::throw_range(
                        &ctx,
                        "fractionalSecondDigits must be \"auto\" or 0-9",
                    ));
                }
                let digits = number.floor() as i128;
                if !(0..=9).contains(&digits) {
                    return Err(Exception::throw_range(
                        &ctx,
                        "fractionalSecondDigits must be \"auto\" or 0-9",
                    ));
                }
                Precision::Digit(digits as u8)
            }
            Some(value) => {
                let name = string_option(&ctx, &value)?;
                if name == "auto" {
                    Precision::Auto
                } else {
                    return Err(Exception::throw_range(
                        &ctx,
                        "fractionalSecondDigits must be \"auto\" or 0-9",
                    ));
                }
            }
        };
        let rounding_mode = rounding_mode_option(&ctx, &object)?;
        let smallest_unit = optional_unit(&ctx, &object, "smallestUnit")?;
        unwrap_temporal(
            &ctx,
            self.inner.as_temporal_string(ToStringRoundingOptions {
                precision,
                smallest_unit,
                rounding_mode,
            }),
        )
    }

    #[qjs(rename = "toJSON")]
    pub fn to_json(&self, ctx: Ctx<'_>) -> Result<String> {
        unwrap_temporal(
            &ctx,
            self.inner
                .as_temporal_string(ToStringRoundingOptions::default()),
        )
    }

    pub fn value_of(&self, ctx: Ctx<'_>) -> Result<()> {
        Err(throw_value_of(&ctx, "Temporal.Duration"))
    }

    #[qjs(prop, rename = PredefinedAtom::SymbolToStringTag, configurable)]
    pub const fn to_string_tag() -> &'static str { "Temporal.Duration" }
}
