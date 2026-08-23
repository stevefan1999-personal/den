use std::str::FromStr;

use rquickjs::{
    atom::PredefinedAtom, class::Trace, function::This, prelude::Opt, prelude::Rest, Ctx, Exception,
    JsLifetime, Object, Result, Value,
};
use temporal_rs::{
    fields::{CalendarFields, DateTimeFields},
    options::{
        DifferenceSettings, Disambiguation, DisplayCalendar, Overflow, RoundingIncrement,
        RoundingMode, RoundingOptions, ToStringRoundingOptions, Unit,
    },
    parsers::Precision,
    partial::{PartialDateTime, PartialDuration, PartialTime},
    Calendar, MonthCode,
};

use crate::{
    convert::{
        get_defined, js_to_string, options_object, ordering_i32, probe_class,
        reject_calendar_or_time_zone, reject_illformed_month_code, require_object, throw_value_of,
        to_integer_if_integral, to_integer_if_integral_i64, to_integer_with_truncation, to_number,
        to_time_zone, unwrap_temporal,
    },
    duration::Duration,
    plain_date::PlainDate,
    plain_month_day::PlainMonthDay,
    plain_time::PlainTime,
    plain_year_month::PlainYearMonth,
    zoned_date_time::ZonedDateTime,
};

#[derive(Trace, JsLifetime, Clone)]
#[rquickjs::class(rename = "PlainDateTime", frozen)]
pub struct PlainDateTime {
    #[qjs(skip_trace)]
    pub(crate) inner: temporal_rs::PlainDateTime,
}

impl PlainDateTime {
    pub(crate) fn wrap(inner: temporal_rs::PlainDateTime) -> Self { Self { inner } }
}

fn range_int(ctx: &Ctx<'_>) -> rquickjs::Error {
    Exception::throw_range(ctx, "integer is out of range")
}

fn i32_from_trunc<'js>(ctx: &Ctx<'js>, value: &Value<'js>) -> Result<i32> {
    i32::try_from(to_integer_with_truncation(ctx, value)?).map_err(|_| range_int(ctx))
}

fn u8_from_trunc<'js>(ctx: &Ctx<'js>, value: &Value<'js>) -> Result<u8> {
    u8::try_from(i32_from_trunc(ctx, value)?).map_err(|_| range_int(ctx))
}

fn u16_from_trunc<'js>(ctx: &Ctx<'js>, value: &Value<'js>) -> Result<u16> {
    u16::try_from(i32_from_trunc(ctx, value)?).map_err(|_| range_int(ctx))
}

fn ctor_required_i32<'js>(ctx: &Ctx<'js>, value: Opt<Value<'js>>) -> Result<i32> {
    let value = value.0.unwrap_or_else(|| Value::new_undefined(ctx.clone()));
    i32_from_trunc(ctx, &value)
}

fn ctor_required_u8<'js>(ctx: &Ctx<'js>, value: Opt<Value<'js>>) -> Result<u8> {
    u8::try_from(ctor_required_i32(ctx, value)?).map_err(|_| range_int(ctx))
}

fn ctor_optional_u8<'js>(ctx: &Ctx<'js>, value: Option<Value<'js>>) -> Result<u8> {
    match value {
        Some(value) if !value.is_undefined() => u8_from_trunc(ctx, &value),
        _ => Ok(0),
    }
}

fn ctor_optional_u16<'js>(ctx: &Ctx<'js>, value: Option<Value<'js>>) -> Result<u16> {
    match value {
        Some(value) if !value.is_undefined() => u16_from_trunc(ctx, &value),
        _ => Ok(0),
    }
}

/// `ToPrimitive` with hint string. `String(object)` in this engine prefers
/// `valueOf`, which breaks GetOption / `ToTemporalMonthCode` observers.
fn to_primitive_hint_string<'js>(ctx: &Ctx<'js>, value: &Value<'js>) -> Result<Value<'js>> {
    let Some(object) = value.as_object() else {
        return Ok(value.clone());
    };
    let exotic: Value = object.get(PredefinedAtom::SymbolToPrimitive)?;
    if !exotic.is_undefined() {
        let Some(func) = exotic.as_function() else {
            return Err(Exception::throw_type(
                ctx,
                "Cannot convert object to primitive value",
            ));
        };
        let primitive: Value = func.call((This(object.clone()), "string"))?;
        if primitive.is_object() {
            return Err(Exception::throw_type(
                ctx,
                "Cannot convert object to primitive value",
            ));
        }
        return Ok(primitive);
    }
    for key in ["toString", "valueOf"] {
        let method: Value = object.get(key)?;
        let Some(func) = method.as_function() else {
            continue;
        };
        let primitive: Value = func.call((This(object.clone()),))?;
        if !primitive.is_object() {
            return Ok(primitive);
        }
    }
    Err(Exception::throw_type(
        ctx,
        "Cannot convert object to primitive value",
    ))
}

/// `ToString`, with hint string on objects (GetOption).
fn temporal_to_string<'js>(ctx: &Ctx<'js>, value: &Value<'js>) -> Result<String> {
    if value.is_symbol() {
        return Err(Exception::throw_type(
            ctx,
            "Cannot convert a Symbol value to a string",
        ));
    }
    if value.is_object() {
        let primitive = to_primitive_hint_string(ctx, value)?;
        return temporal_to_string(ctx, &primitive);
    }
    js_to_string(ctx, value)
}

/// `ToTemporalMonthCode`: ToPrimitive hint string, then require a String.
fn to_month_code_string<'js>(ctx: &Ctx<'js>, value: &Value<'js>) -> Result<String> {
    let primitive = to_primitive_hint_string(ctx, value)?;
    if !primitive.is_string() {
        return Err(Exception::throw_type(ctx, "monthCode must be a string"));
    }
    let code = primitive.get::<String>()?;
    reject_illformed_month_code(ctx, &code)?;
    Ok(code)
}

fn get_overflow<'js>(ctx: &Ctx<'js>, options: &Option<Object<'js>>) -> Result<Option<Overflow>> {
    let Some(object) = options else {
        return Ok(None);
    };
    let Some(value) = get_defined(object, "overflow")? else {
        return Ok(None);
    };
    let name = temporal_to_string(ctx, &value)?;
    Overflow::from_str(&name)
        .map(Some)
        .map_err(|_| Exception::throw_range(ctx, "invalid overflow option"))
}

fn get_unit_option<'js>(
    ctx: &Ctx<'js>, object: &Object<'js>, key: &str,
) -> Result<Option<Unit>> {
    let Some(value) = get_defined(object, key)? else {
        return Ok(None);
    };
    let name = temporal_to_string(ctx, &value)?;
    Unit::from_str(&name)
        .map(Some)
        .map_err(|_| Exception::throw_range(ctx, "invalid Temporal unit"))
}

fn get_rounding_mode_option<'js>(
    ctx: &Ctx<'js>, object: &Object<'js>,
) -> Result<Option<RoundingMode>> {
    let Some(value) = get_defined(object, "roundingMode")? else {
        return Ok(None);
    };
    let name = temporal_to_string(ctx, &value)?;
    RoundingMode::from_str(&name)
        .map(Some)
        .map_err(|_| Exception::throw_range(ctx, "invalid roundingMode"))
}

fn get_rounding_increment<'js>(
    ctx: &Ctx<'js>, object: &Object<'js>,
) -> Result<Option<RoundingIncrement>> {
    let Some(value) = get_defined(object, "roundingIncrement")? else {
        return Ok(None);
    };
    let number = to_number(ctx, &value)?;
    unwrap_temporal(ctx, RoundingIncrement::try_from(number)).map(Some)
}

fn get_fractional_second_digits<'js>(
    ctx: &Ctx<'js>, object: &Object<'js>,
) -> Result<Precision> {
    let Some(value) = get_defined(object, "fractionalSecondDigits")? else {
        return Ok(Precision::Auto);
    };
    if value.is_number() {
        let number = to_number(ctx, &value)?;
        if !number.is_finite() {
            return Err(Exception::throw_range(
                ctx,
                "fractionalSecondDigits must be finite",
            ));
        }
        let digits = number.floor() as i128;
        if !(0..=9).contains(&digits) {
            return Err(Exception::throw_range(
                ctx,
                "fractionalSecondDigits must be \"auto\" or 0-9",
            ));
        }
        return Ok(Precision::Digit(digits as u8));
    }
    let name = temporal_to_string(ctx, &value)?;
    if name == "auto" {
        Ok(Precision::Auto)
    } else {
        Err(Exception::throw_range(
            ctx,
            "fractionalSecondDigits must be \"auto\" or 0-9",
        ))
    }
}

fn difference_settings<'js>(
    ctx: &Ctx<'js>, options: Opt<Value<'js>>,
) -> Result<DifferenceSettings> {
    let object = options_object(ctx, options)?;
    let Some(object) = object else {
        return Ok(DifferenceSettings::default());
    };
    let largest_unit = get_unit_option(ctx, &object, "largestUnit")?;
    let increment = get_rounding_increment(ctx, &object)?;
    let rounding_mode = get_rounding_mode_option(ctx, &object)?;
    let smallest_unit = get_unit_option(ctx, &object, "smallestUnit")?;
    let mut settings = DifferenceSettings::default();
    settings.largest_unit = largest_unit;
    settings.smallest_unit = smallest_unit;
    settings.rounding_mode = rounding_mode;
    settings.increment = increment;
    Ok(settings)
}

fn calendar_from_slots<'js>(ctx: &Ctx<'js>, value: &Value<'js>) -> Option<Calendar> {
    if let Some(date_time) = probe_class::<PlainDateTime>(ctx, value) {
        return Some(date_time.inner.calendar().clone());
    }
    if let Some(date) = probe_class::<PlainDate>(ctx, value) {
        return Some(date.inner.calendar().clone());
    }
    if let Some(year_month) = probe_class::<PlainYearMonth>(ctx, value) {
        return Some(year_month.inner.calendar().clone());
    }
    if let Some(month_day) = probe_class::<PlainMonthDay>(ctx, value) {
        return Some(month_day.inner.calendar().clone());
    }
    if let Some(zoned) = probe_class::<ZonedDateTime>(ctx, value) {
        return Some(zoned.inner.calendar().clone());
    }
    None
}

fn has_calendar_or_time_zone_slot<'js>(ctx: &Ctx<'js>, value: &Value<'js>) -> bool {
    probe_class::<PlainDateTime>(ctx, value).is_some()
        || probe_class::<PlainDate>(ctx, value).is_some()
        || probe_class::<PlainYearMonth>(ctx, value).is_some()
        || probe_class::<PlainMonthDay>(ctx, value).is_some()
        || probe_class::<ZonedDateTime>(ctx, value).is_some()
        || probe_class::<PlainTime>(ctx, value).is_some()
}

/// `ParseTemporalCalendarString` + canonicalize (bags, `withCalendar`).
fn to_calendar_like<'js>(ctx: &Ctx<'js>, value: &Value<'js>) -> Result<Calendar> {
    if let Some(calendar) = calendar_from_slots(ctx, value) {
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
    if let Some(calendar) = calendar_from_slots(ctx, value) {
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

fn get_calendar_with_iso_default<'js>(
    ctx: &Ctx<'js>, object: &Object<'js>,
) -> Result<Calendar> {
    match get_defined(object, "calendar")? {
        None => Ok(Calendar::ISO),
        Some(value) => to_calendar_like(ctx, &value),
    }
}

fn truncated_i32_field<'js>(
    ctx: &Ctx<'js>, object: &Object<'js>, key: &str,
) -> Result<Option<i32>> {
    let value: Value = object.get(key)?;
    if value.is_undefined() {
        return Ok(None);
    }
    i32_from_trunc(ctx, &value).map(Some)
}

fn truncated_u8_field<'js>(
    ctx: &Ctx<'js>, object: &Object<'js>, key: &str,
) -> Result<Option<u8>> {
    let value: Value = object.get(key)?;
    if value.is_undefined() {
        return Ok(None);
    }
    u8_from_trunc(ctx, &value).map(Some)
}

fn truncated_u16_field<'js>(
    ctx: &Ctx<'js>, object: &Object<'js>, key: &str,
) -> Result<Option<u16>> {
    let value: Value = object.get(key)?;
    if value.is_undefined() {
        return Ok(None);
    }
    u16_from_trunc(ctx, &value).map(Some)
}

fn integral_i64_field<'js>(
    ctx: &Ctx<'js>, object: &Object<'js>, key: &str,
) -> Result<Option<i64>> {
    let value: Value = object.get(key)?;
    if value.is_undefined() {
        return Ok(None);
    }
    to_integer_if_integral_i64(ctx, &value).map(Some)
}

fn integral_i128_field<'js>(
    ctx: &Ctx<'js>, object: &Object<'js>, key: &str,
) -> Result<Option<i128>> {
    let value: Value = object.get(key)?;
    if value.is_undefined() {
        return Ok(None);
    }
    to_integer_if_integral(ctx, &value).map(Some)
}

/// PrepareTemporalFields Get order: day, hour, micro, milli, minute, month,
/// monthCode, nano, second, year.
fn datetime_fields_from_object<'js>(
    ctx: &Ctx<'js>, object: &Object<'js>,
) -> Result<(DateTimeFields, Option<String>)> {
    let day = truncated_u8_field(ctx, object, "day")?;
    let hour = truncated_u8_field(ctx, object, "hour")?;
    let microsecond = truncated_u16_field(ctx, object, "microsecond")?;
    let millisecond = truncated_u16_field(ctx, object, "millisecond")?;
    let minute = truncated_u8_field(ctx, object, "minute")?;
    let month = truncated_u8_field(ctx, object, "month")?;
    let month_code_text = match get_defined(object, "monthCode")? {
        None => None,
        Some(value) => Some(to_month_code_string(ctx, &value)?),
    };
    let nanosecond = truncated_u16_field(ctx, object, "nanosecond")?;
    let second = truncated_u8_field(ctx, object, "second")?;
    let year = truncated_i32_field(ctx, object, "year")?;
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

fn apply_month_code<'js>(
    ctx: &Ctx<'js>, mut fields: DateTimeFields, month_code: Option<String>,
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
        let options = options_object(ctx, options)?;
        let _overflow = get_overflow(ctx, &options)?;
        return Ok(date_time.inner);
    }
    if let Some(zoned) = probe_class::<ZonedDateTime>(ctx, value) {
        let options = options_object(ctx, options)?;
        let _overflow = get_overflow(ctx, &options)?;
        return Ok(zoned.inner.to_plain_date_time());
    }
    if let Some(date) = probe_class::<PlainDate>(ctx, value) {
        let options = options_object(ctx, options)?;
        let _overflow = get_overflow(ctx, &options)?;
        return unwrap_temporal(ctx, date.inner.to_plain_date_time(None));
    }
    if value.is_string() {
        let string = value.get::<String>()?;
        let parsed = unwrap_temporal(ctx, temporal_rs::PlainDateTime::from_utf8(string.as_bytes()))?;
        let options = options_object(ctx, options)?;
        let _overflow = get_overflow(ctx, &options)?;
        return Ok(parsed);
    }
    let object = require_object(ctx, value, "cannot convert value to Temporal.PlainDateTime")?;
    let calendar = get_calendar_with_iso_default(ctx, &object)?;
    let (fields, month_code) = datetime_fields_from_object(ctx, &object)?;
    let options = options_object(ctx, options)?;
    let overflow = get_overflow(ctx, &options)?;
    let fields = apply_month_code(ctx, fields, month_code)?;
    unwrap_temporal(
        ctx,
        temporal_rs::PlainDateTime::from_partial(PartialDateTime { fields, calendar }, overflow),
    )
}

fn to_duration_like<'js>(
    ctx: &Ctx<'js>, value: &Value<'js>,
) -> Result<temporal_rs::Duration> {
    if let Some(duration) = probe_class::<Duration>(ctx, value) {
        return Ok(duration.inner);
    }
    if value.is_string() {
        let string = value.get::<String>()?;
        return unwrap_temporal(ctx, temporal_rs::Duration::from_utf8(string.as_bytes()));
    }
    let object = require_object(ctx, value, "cannot convert value to Temporal.Duration")?;
    let days = integral_i64_field(ctx, &object, "days")?;
    let hours = integral_i64_field(ctx, &object, "hours")?;
    let microseconds = integral_i128_field(ctx, &object, "microseconds")?;
    let milliseconds = integral_i64_field(ctx, &object, "milliseconds")?;
    let minutes = integral_i64_field(ctx, &object, "minutes")?;
    let months = integral_i64_field(ctx, &object, "months")?;
    let nanoseconds = integral_i128_field(ctx, &object, "nanoseconds")?;
    let seconds = integral_i64_field(ctx, &object, "seconds")?;
    let weeks = integral_i64_field(ctx, &object, "weeks")?;
    let years = integral_i64_field(ctx, &object, "years")?;
    unwrap_temporal(
        ctx,
        temporal_rs::Duration::from_partial_duration(PartialDuration {
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
        }),
    )
}

fn to_plain_time_like<'js>(
    ctx: &Ctx<'js>, value: &Value<'js>,
) -> Result<temporal_rs::PlainTime> {
    if let Some(time) = probe_class::<PlainTime>(ctx, value) {
        return Ok(time.inner);
    }
    if let Some(date_time) = probe_class::<PlainDateTime>(ctx, value) {
        return Ok(date_time.inner.to_plain_time());
    }
    if let Some(zoned) = probe_class::<ZonedDateTime>(ctx, value) {
        return Ok(zoned.inner.to_plain_time());
    }
    if value.is_string() {
        let string = value.get::<String>()?;
        return unwrap_temporal(ctx, temporal_rs::PlainTime::from_utf8(string.as_bytes()));
    }
    let object = require_object(ctx, value, "cannot convert value to Temporal.PlainTime")?;
    let hour = truncated_u8_field(ctx, &object, "hour")?;
    let microsecond = truncated_u16_field(ctx, &object, "microsecond")?;
    let millisecond = truncated_u16_field(ctx, &object, "millisecond")?;
    let minute = truncated_u8_field(ctx, &object, "minute")?;
    let nanosecond = truncated_u16_field(ctx, &object, "nanosecond")?;
    let second = truncated_u8_field(ctx, &object, "second")?;
    let partial = PartialTime {
        hour,
        minute,
        second,
        millisecond,
        microsecond,
        nanosecond,
    };
    if partial.is_empty() {
        return Err(Exception::throw_type(
            ctx,
            "time bag must have at least one time field",
        ));
    }
    unwrap_temporal(ctx, temporal_rs::PlainTime::from_partial(partial, None))
}

fn rounding_from_value<'js>(ctx: &Ctx<'js>, options: Value<'js>) -> Result<RoundingOptions> {
    if options.is_string() {
        let mut rounding = RoundingOptions::default();
        rounding.largest_unit = Some(Unit::Auto);
        rounding.smallest_unit = Some({
            let name = temporal_to_string(ctx, &options)?;
            Unit::from_str(&name)
                .map_err(|_| Exception::throw_range(ctx, "invalid Temporal unit"))?
        });
        return Ok(rounding);
    }
    let object = require_object(ctx, &options, "options must be an object")?;
    let increment = get_rounding_increment(ctx, &object)?;
    let rounding_mode = get_rounding_mode_option(ctx, &object)?;
    let smallest_unit = get_unit_option(ctx, &object, "smallestUnit")?;
    let mut rounding = RoundingOptions::default();
    rounding.largest_unit = Some(Unit::Auto);
    rounding.smallest_unit = smallest_unit;
    rounding.rounding_mode = rounding_mode;
    rounding.increment = increment;
    Ok(rounding)
}

fn get_disambiguation<'js>(
    ctx: &Ctx<'js>, options: &Option<Object<'js>>,
) -> Result<Disambiguation> {
    let Some(object) = options else {
        return Ok(Disambiguation::Compatible);
    };
    let Some(value) = get_defined(object, "disambiguation")? else {
        return Ok(Disambiguation::Compatible);
    };
    let name = temporal_to_string(ctx, &value)?;
    Disambiguation::from_str(&name)
        .map_err(|_| Exception::throw_range(ctx, "invalid disambiguation"))
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
        let hour = ctor_optional_u8(&ctx, rest.next())?;
        let minute = ctor_optional_u8(&ctx, rest.next())?;
        let second = ctor_optional_u8(&ctx, rest.next())?;
        let millisecond = ctor_optional_u16(&ctx, rest.next())?;
        let microsecond = ctor_optional_u16(&ctx, rest.next())?;
        let nanosecond = ctor_optional_u16(&ctx, rest.next())?;
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
        Ok(ordering_i32(left.compare_iso(&right)))
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
        let duration = to_duration_like(&ctx, &duration_like)?;
        let overflow = get_overflow(&ctx, &options_object(&ctx, options)?)?;
        unwrap_temporal(&ctx, self.inner.add(&duration, overflow)).map(Self::wrap)
    }

    pub fn subtract<'js>(
        &self, duration_like: Value<'js>, options: Opt<Value<'js>>, ctx: Ctx<'js>,
    ) -> Result<Self> {
        let duration = to_duration_like(&ctx, &duration_like)?;
        let overflow = get_overflow(&ctx, &options_object(&ctx, options)?)?;
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
        reject_calendar_or_time_zone(&ctx, &object, "calendar is not allowed in with()", "timeZone is not allowed in with()")?;
        let (fields, month_code) = datetime_fields_from_object(&ctx, &object)?;
        let overflow = get_overflow(&ctx, &options_object(&ctx, options)?)?;
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
            Some(value) if !value.is_undefined() => Some(to_plain_time_like(&ctx, &value)?),
            _ => None,
        };
        unwrap_temporal(&ctx, self.inner.with_time(time)).map(Self::wrap)
    }

    pub fn round<'js>(&self, options: Value<'js>, ctx: Ctx<'js>) -> Result<Self> {
        let rounding = rounding_from_value(&ctx, options)?;
        unwrap_temporal(&ctx, self.inner.round(rounding)).map(Self::wrap)
    }

    pub fn to_plain_date(&self) -> PlainDate { PlainDate::wrap(self.inner.to_plain_date()) }

    pub fn to_plain_time(&self) -> PlainTime { PlainTime::wrap(self.inner.to_plain_time()) }

    pub fn to_zoned_date_time<'js>(
        &self, time_zone: Value<'js>, options: Opt<Value<'js>>, ctx: Ctx<'js>,
    ) -> Result<ZonedDateTime> {
        let zone = to_time_zone(&ctx, &time_zone)?;
        let disambiguation = get_disambiguation(&ctx, &options_object(&ctx, options)?)?;
        unwrap_temporal(&ctx, self.inner.to_zoned_date_time(zone, disambiguation))
            .map(ZonedDateTime::wrap)
    }

    pub fn to_string<'js>(&self, options: Opt<Value<'js>>, ctx: Ctx<'js>) -> Result<String> {
        let object = options_object(&ctx, options)?;
        let (rounding, display) = match object {
            None => (ToStringRoundingOptions::default(), DisplayCalendar::Auto),
            Some(object) => {
                let display = match get_defined(&object, "calendarName")? {
                    None => DisplayCalendar::Auto,
                    Some(value) => {
                        let name = temporal_to_string(&ctx, &value)?;
                        DisplayCalendar::from_str(&name).map_err(|_| {
                            Exception::throw_range(&ctx, "invalid calendarName option")
                        })?
                    }
                };
                let precision = get_fractional_second_digits(&ctx, &object)?;
                let rounding_mode = get_rounding_mode_option(&ctx, &object)?;
                let smallest_unit = get_unit_option(&ctx, &object, "smallestUnit")?;
                (
                    ToStringRoundingOptions {
                        precision,
                        smallest_unit,
                        rounding_mode,
                    },
                    display,
                )
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

    pub fn to_locale_string<'js>(
        &self, _locales: Opt<Value<'js>>, _options: Opt<Value<'js>>, ctx: Ctx<'js>,
    ) -> Result<String> {
        self.to_json(ctx)
    }

    pub fn value_of(&self, ctx: Ctx<'_>) -> Result<()> {
        Err(throw_value_of(&ctx, "Temporal.PlainDateTime"))
    }

    #[qjs(prop, rename = PredefinedAtom::SymbolToStringTag, configurable)]
    pub fn to_string_tag() -> &'static str { "Temporal.PlainDateTime" }
}
