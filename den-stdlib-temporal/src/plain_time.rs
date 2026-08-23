use std::str::FromStr;

use rquickjs::{
    Ctx, Exception, JsLifetime, Object, Result, Value, atom::PredefinedAtom, class::Trace,
    function::This, prelude::Opt,
};
use temporal_rs::{
    options::{
        DifferenceSettings, Overflow, RoundingIncrement, RoundingMode, RoundingOptions,
        ToStringRoundingOptions, Unit,
    },
    parsers::Precision,
    partial::{PartialDuration, PartialTime},
};

use crate::{
    convert::{
        js_to_string, optional_truncated_u16, optional_truncated_u8, ordering_i32, probe_class,
        throw_value_of, to_integer_if_integral, to_integer_if_integral_i64,
        to_integer_with_truncation, to_number, unwrap_temporal,
    },
    duration::Duration,
    plain_date::PlainDate,
    plain_date_time::PlainDateTime,
    plain_month_day::PlainMonthDay,
    plain_year_month::PlainYearMonth,
    zoned_date_time::ZonedDateTime,
};

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
        hour: Opt<Value<'js>>, minute: Opt<Value<'js>>, second: Opt<Value<'js>>,
        millisecond: Opt<Value<'js>>, microsecond: Opt<Value<'js>>, nanosecond: Opt<Value<'js>>,
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
        if let Some(time) = existing_plain_time(&ctx, &item) {
            let _overflow = overflow_from_options(&ctx, options)?;
            return Ok(Self::wrap(time));
        }
        if item.is_string() {
            let time = parse_plain_time(&ctx, &item)?;
            let _overflow = overflow_from_options(&ctx, options)?;
            return Ok(Self::wrap(time));
        }
        let object = require_object(&ctx, &item, "cannot convert value to Temporal.PlainTime")?;
        let record = to_time_record(&ctx, &object)?;
        let overflow = overflow_from_options(&ctx, options)?;
        time_from_record(&ctx, record, overflow).map(Self::wrap)
    }

    #[qjs(static)]
    pub fn compare<'js>(one: Value<'js>, two: Value<'js>, ctx: Ctx<'js>) -> Result<i32> {
        let left = to_temporal_time(&ctx, &one)?;
        let right = to_temporal_time(&ctx, &two)?;
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
        let duration = to_duration_like(&ctx, &duration_like)?;
        unwrap_temporal(&ctx, self.inner.add(&duration)).map(Self::wrap)
    }

    pub fn subtract<'js>(&self, duration_like: Value<'js>, ctx: Ctx<'js>) -> Result<Self> {
        let duration = to_duration_like(&ctx, &duration_like)?;
        unwrap_temporal(&ctx, self.inner.subtract(&duration)).map(Self::wrap)
    }

    pub fn until<'js>(
        &self, other: Value<'js>, options: Opt<Value<'js>>, ctx: Ctx<'js>,
    ) -> Result<Duration> {
        let other = to_temporal_time(&ctx, &other)?;
        let settings = difference_settings(&ctx, options)?;
        unwrap_temporal(&ctx, self.inner.until(&other, settings)).map(Duration::wrap)
    }

    pub fn since<'js>(
        &self, other: Value<'js>, options: Opt<Value<'js>>, ctx: Ctx<'js>,
    ) -> Result<Duration> {
        let other = to_temporal_time(&ctx, &other)?;
        let settings = difference_settings(&ctx, options)?;
        unwrap_temporal(&ctx, self.inner.since(&other, settings)).map(Duration::wrap)
    }

    pub fn round<'js>(&self, round_to: Value<'js>, ctx: Ctx<'js>) -> Result<Self> {
        if round_to.is_undefined() {
            return Err(Exception::throw_type(&ctx, "roundTo is required"));
        }
        let rounding = if round_to.is_string() {
            let mut rounding = RoundingOptions::default();
            rounding.smallest_unit = Some(option_unit(&ctx, &round_to)?);
            rounding
        } else {
            let object = require_object(&ctx, &round_to, "roundTo must be an object or string")?;
            time_rounding_options(&ctx, &object)?
        };
        unwrap_temporal(&ctx, self.inner.round(rounding)).map(Self::wrap)
    }

    pub fn with<'js>(
        &self, temporal_time_like: Value<'js>, options: Opt<Value<'js>>, ctx: Ctx<'js>,
    ) -> Result<Self> {
        let object = require_object(&ctx, &temporal_time_like, "with() requires a property bag")?;
        reject_calendar_or_time_zone(&ctx, &object)?;
        let record = to_time_record(&ctx, &object)?;
        let overflow = overflow_from_options(&ctx, options)?;
        let partial = partial_from_record(&ctx, record, overflow)?;
        unwrap_temporal(&ctx, self.inner.with(partial, Some(overflow))).map(Self::wrap)
    }

    pub fn equals<'js>(&self, other: Value<'js>, ctx: Ctx<'js>) -> Result<bool> {
        Ok(self.inner == to_temporal_time(&ctx, &other)?)
    }

    pub fn to_string<'js>(&self, options: Opt<Value<'js>>, ctx: Ctx<'js>) -> Result<String> {
        let rounding = string_rounding_options(&ctx, options)?;
        unwrap_temporal(&ctx, self.inner.to_ixdtf_string(rounding))
    }

    #[qjs(rename = "toJSON")]
    pub fn to_json(&self, ctx: Ctx<'_>) -> Result<String> {
        unwrap_temporal(
            &ctx,
            self.inner
                .to_ixdtf_string(ToStringRoundingOptions::default()),
        )
    }

    pub fn to_locale_string(&self, ctx: Ctx<'_>) -> Result<String> {
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

#[derive(Clone, Copy, Default, PartialEq, Eq)]
struct TimeRecord {
    hour: Option<i128>,
    minute: Option<i128>,
    second: Option<i128>,
    millisecond: Option<i128>,
    microsecond: Option<i128>,
    nanosecond: Option<i128>,
}

impl TimeRecord {
    fn is_empty(self) -> bool {
        self == Self::default()
    }
}

fn existing_plain_time<'js>(ctx: &Ctx<'js>, value: &Value<'js>) -> Option<temporal_rs::PlainTime> {
    if let Some(time) = probe_class::<PlainTime>(ctx, value) {
        return Some(time.inner);
    }
    if let Some(date_time) = probe_class::<PlainDateTime>(ctx, value) {
        return Some(date_time.inner.to_plain_time());
    }
    if let Some(zoned) = probe_class::<ZonedDateTime>(ctx, value) {
        return Some(zoned.inner.to_plain_time());
    }
    None
}

fn parse_plain_time<'js>(ctx: &Ctx<'js>, value: &Value<'js>) -> Result<temporal_rs::PlainTime> {
    let string = value.get::<String>()?;
    unwrap_temporal(ctx, temporal_rs::PlainTime::from_utf8(string.as_bytes()))
}

fn to_temporal_time<'js>(ctx: &Ctx<'js>, value: &Value<'js>) -> Result<temporal_rs::PlainTime> {
    if let Some(time) = existing_plain_time(ctx, value) {
        return Ok(time);
    }
    if value.is_string() {
        return parse_plain_time(ctx, value);
    }
    let object = require_object(ctx, value, "cannot convert value to Temporal.PlainTime")?;
    let record = to_time_record(ctx, &object)?;
    time_from_record(ctx, record, Overflow::Constrain)
}

fn require_object<'js>(ctx: &Ctx<'js>, value: &Value<'js>, message: &str) -> Result<Object<'js>> {
    match value.as_object() {
        Some(object) => Ok(object.clone()),
        None => Err(Exception::throw_type(ctx, message)),
    }
}

fn options_object<'js>(ctx: &Ctx<'js>, options: Opt<Value<'js>>) -> Result<Option<Object<'js>>> {
    match options.0 {
        None => Ok(None),
        Some(value) if value.is_undefined() => Ok(None),
        Some(value) => require_object(ctx, &value, "options must be an object").map(Some),
    }
}

fn overflow_from_options<'js>(ctx: &Ctx<'js>, options: Opt<Value<'js>>) -> Result<Overflow> {
    overflow_from_object(ctx, options_object(ctx, options)?.as_ref())
}

fn overflow_from_object<'js>(ctx: &Ctx<'js>, options: Option<&Object<'js>>) -> Result<Overflow> {
    let Some(options) = options else {
        return Ok(Overflow::Constrain);
    };
    let value = get_prop(options, "overflow")?;
    if value.is_undefined() {
        Ok(Overflow::Constrain)
    } else {
        option_overflow(ctx, &value)
    }
}

fn get_prop<'js>(object: &Object<'js>, key: &str) -> Result<Value<'js>> {
    object.get(key)
}

fn optional_truncated_field<'js>(
    ctx: &Ctx<'js>, object: &Object<'js>, key: &str,
) -> Result<Option<i128>> {
    let value = get_prop(object, key)?;
    if value.is_undefined() {
        Ok(None)
    } else {
        to_integer_with_truncation(ctx, &value).map(Some)
    }
}

fn optional_integral_i64<'js>(
    ctx: &Ctx<'js>, object: &Object<'js>, key: &str,
) -> Result<Option<i64>> {
    let value = get_prop(object, key)?;
    if value.is_undefined() {
        Ok(None)
    } else {
        to_integer_if_integral_i64(ctx, &value).map(Some)
    }
}

fn optional_integral_i128<'js>(
    ctx: &Ctx<'js>, object: &Object<'js>, key: &str,
) -> Result<Option<i128>> {
    let value = get_prop(object, key)?;
    if value.is_undefined() {
        Ok(None)
    } else {
        to_integer_if_integral(ctx, &value).map(Some)
    }
}

/// `ToTemporalTimeRecord`: Get time units in alphabetical order.
fn to_time_record<'js>(ctx: &Ctx<'js>, object: &Object<'js>) -> Result<TimeRecord> {
    let hour = optional_truncated_field(ctx, object, "hour")?;
    let microsecond = optional_truncated_field(ctx, object, "microsecond")?;
    let millisecond = optional_truncated_field(ctx, object, "millisecond")?;
    let minute = optional_truncated_field(ctx, object, "minute")?;
    let nanosecond = optional_truncated_field(ctx, object, "nanosecond")?;
    let second = optional_truncated_field(ctx, object, "second")?;
    let record = TimeRecord {
        hour,
        minute,
        second,
        millisecond,
        microsecond,
        nanosecond,
    };
    if record.is_empty() {
        return Err(Exception::throw_type(
            ctx,
            "Temporal.PlainTime requires a time unit",
        ));
    }
    Ok(record)
}

fn reject_calendar_or_time_zone<'js>(ctx: &Ctx<'js>, object: &Object<'js>) -> Result<()> {
    let value = Value::from_object(object.clone());
    if probe_class::<PlainTime>(ctx, &value).is_some()
        || probe_class::<PlainDate>(ctx, &value).is_some()
        || probe_class::<PlainDateTime>(ctx, &value).is_some()
        || probe_class::<PlainYearMonth>(ctx, &value).is_some()
        || probe_class::<PlainMonthDay>(ctx, &value).is_some()
        || probe_class::<ZonedDateTime>(ctx, &value).is_some()
    {
        return Err(Exception::throw_type(
            ctx,
            "calendar or time zone objects are not valid for Temporal.PlainTime.with",
        ));
    }
    let calendar = get_prop(object, "calendar")?;
    if !calendar.is_undefined() {
        return Err(Exception::throw_type(
            ctx,
            "calendar is not allowed on Temporal.PlainTime.with",
        ));
    }
    let time_zone = get_prop(object, "timeZone")?;
    if !time_zone.is_undefined() {
        return Err(Exception::throw_type(
            ctx,
            "timeZone is not allowed on Temporal.PlainTime.with",
        ));
    }
    Ok(())
}

fn regulate_field(ctx: &Ctx<'_>, value: i128, max: i128, overflow: Overflow) -> Result<u16> {
    let regulated = match overflow {
        Overflow::Constrain => value.clamp(0, max),
        Overflow::Reject if (0..=max).contains(&value) => value,
        Overflow::Reject => {
            return Err(Exception::throw_range(ctx, "time value out of range"));
        }
    };
    u16::try_from(regulated).map_err(|_| Exception::throw_range(ctx, "time value out of range"))
}

fn partial_from_record(
    ctx: &Ctx<'_>, record: TimeRecord, overflow: Overflow,
) -> Result<PartialTime> {
    Ok(PartialTime {
        hour: record
            .hour
            .map(|value| regulate_field(ctx, value, 23, overflow).map(|value| value as u8))
            .transpose()?,
        minute: record
            .minute
            .map(|value| regulate_field(ctx, value, 59, overflow).map(|value| value as u8))
            .transpose()?,
        second: record
            .second
            .map(|value| regulate_field(ctx, value, 59, overflow).map(|value| value as u8))
            .transpose()?,
        millisecond: record
            .millisecond
            .map(|value| regulate_field(ctx, value, 999, overflow))
            .transpose()?,
        microsecond: record
            .microsecond
            .map(|value| regulate_field(ctx, value, 999, overflow))
            .transpose()?,
        nanosecond: record
            .nanosecond
            .map(|value| regulate_field(ctx, value, 999, overflow))
            .transpose()?,
    })
}

fn time_from_record(
    ctx: &Ctx<'_>, record: TimeRecord, overflow: Overflow,
) -> Result<temporal_rs::PlainTime> {
    let partial = partial_from_record(ctx, record, overflow)?;
    unwrap_temporal(
        ctx,
        temporal_rs::PlainTime::from_partial(partial, Some(overflow)),
    )
}

fn to_duration_like<'js>(ctx: &Ctx<'js>, value: &Value<'js>) -> Result<temporal_rs::Duration> {
    if let Some(duration) = probe_class::<Duration>(ctx, value) {
        return Ok(duration.inner);
    }
    if value.is_string() {
        let string = value.get::<String>()?;
        return unwrap_temporal(ctx, temporal_rs::Duration::from_utf8(string.as_bytes()));
    }
    let object = require_object(ctx, value, "cannot convert value to Temporal.Duration")?;
    let days = optional_integral_i64(ctx, &object, "days")?;
    let hours = optional_integral_i64(ctx, &object, "hours")?;
    let microseconds = optional_integral_i128(ctx, &object, "microseconds")?;
    let milliseconds = optional_integral_i64(ctx, &object, "milliseconds")?;
    let minutes = optional_integral_i64(ctx, &object, "minutes")?;
    let months = optional_integral_i64(ctx, &object, "months")?;
    let nanoseconds = optional_integral_i128(ctx, &object, "nanoseconds")?;
    let seconds = optional_integral_i64(ctx, &object, "seconds")?;
    let weeks = optional_integral_i64(ctx, &object, "weeks")?;
    let years = optional_integral_i64(ctx, &object, "years")?;
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

fn difference_settings<'js>(
    ctx: &Ctx<'js>, options: Opt<Value<'js>>,
) -> Result<DifferenceSettings> {
    let object = options_object(ctx, options)?;
    let mut settings = DifferenceSettings::default();
    let Some(object) = object.as_ref() else {
        return Ok(settings);
    };
    let largest_unit = get_prop(object, "largestUnit")?;
    if !largest_unit.is_undefined() {
        settings.largest_unit = Some(option_unit(ctx, &largest_unit)?);
    }
    let increment = get_prop(object, "roundingIncrement")?;
    if !increment.is_undefined() {
        let number = to_number(ctx, &increment)?;
        settings.increment = Some(unwrap_temporal(ctx, RoundingIncrement::try_from(number))?);
    }
    let rounding_mode = get_prop(object, "roundingMode")?;
    if !rounding_mode.is_undefined() {
        settings.rounding_mode = Some(option_rounding_mode(ctx, &rounding_mode)?);
    }
    let smallest_unit = get_prop(object, "smallestUnit")?;
    if !smallest_unit.is_undefined() {
        settings.smallest_unit = Some(option_unit(ctx, &smallest_unit)?);
    }
    Ok(settings)
}

fn time_rounding_options<'js>(ctx: &Ctx<'js>, object: &Object<'js>) -> Result<RoundingOptions> {
    let increment = get_prop(object, "roundingIncrement")?;
    let increment = if increment.is_undefined() {
        None
    } else {
        let number = to_number(ctx, &increment)?;
        Some(unwrap_temporal(ctx, RoundingIncrement::try_from(number))?)
    };
    let rounding_mode = get_prop(object, "roundingMode")?;
    let rounding_mode = if rounding_mode.is_undefined() {
        None
    } else {
        Some(option_rounding_mode(ctx, &rounding_mode)?)
    };
    let smallest_unit = get_prop(object, "smallestUnit")?;
    let smallest_unit = if smallest_unit.is_undefined() {
        None
    } else {
        Some(option_unit(ctx, &smallest_unit)?)
    };
    let mut rounding = RoundingOptions::default();
    rounding.smallest_unit = smallest_unit;
    rounding.rounding_mode = rounding_mode;
    rounding.increment = increment;
    Ok(rounding)
}

fn string_rounding_options<'js>(
    ctx: &Ctx<'js>, options: Opt<Value<'js>>,
) -> Result<ToStringRoundingOptions> {
    let object = options_object(ctx, options)?;
    let Some(object) = object.as_ref() else {
        return Ok(ToStringRoundingOptions::default());
    };
    let fractional = get_prop(object, "fractionalSecondDigits")?;
    let precision = if fractional.is_undefined() {
        Precision::Auto
    } else {
        fractional_second_digits(ctx, &fractional)?
    };
    let rounding_mode = get_prop(object, "roundingMode")?;
    let rounding_mode = if rounding_mode.is_undefined() {
        None
    } else {
        Some(option_rounding_mode(ctx, &rounding_mode)?)
    };
    let smallest_unit = get_prop(object, "smallestUnit")?;
    let smallest_unit = if smallest_unit.is_undefined() {
        None
    } else {
        Some(option_unit(ctx, &smallest_unit)?)
    };
    Ok(ToStringRoundingOptions {
        precision,
        smallest_unit,
        rounding_mode,
    })
}

fn fractional_second_digits<'js>(ctx: &Ctx<'js>, value: &Value<'js>) -> Result<Precision> {
    if value.is_number() {
        let number = to_number(ctx, value)?;
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
    let name = to_option_string(ctx, value)?;
    if name == "auto" {
        Ok(Precision::Auto)
    } else {
        Err(Exception::throw_range(
            ctx,
            "fractionalSecondDigits must be \"auto\" or 0-9",
        ))
    }
}

fn option_unit<'js>(ctx: &Ctx<'js>, value: &Value<'js>) -> Result<Unit> {
    let name = to_option_string(ctx, value)?;
    Unit::from_str(&name).map_err(|_| Exception::throw_range(ctx, "invalid Temporal unit"))
}

fn option_rounding_mode<'js>(ctx: &Ctx<'js>, value: &Value<'js>) -> Result<RoundingMode> {
    let name = to_option_string(ctx, value)?;
    RoundingMode::from_str(&name).map_err(|_| Exception::throw_range(ctx, "invalid roundingMode"))
}

fn option_overflow<'js>(ctx: &Ctx<'js>, value: &Value<'js>) -> Result<Overflow> {
    let name = to_option_string(ctx, value)?;
    Overflow::from_str(&name).map_err(|_| Exception::throw_range(ctx, "invalid overflow option"))
}

fn to_option_string<'js>(ctx: &Ctx<'js>, value: &Value<'js>) -> Result<String> {
    if value.is_symbol() {
        return Err(Exception::throw_type(
            ctx,
            "cannot convert Symbol to a string",
        ));
    }
    if let Some(string) = value.as_string() {
        return string.to_string();
    }
    if let Some(object) = value.as_object() {
        if let Some(primitive) = ordinary_to_primitive_string(object)? {
            return primitive_to_string(ctx, &primitive);
        }
    }
    js_to_string(ctx, value)
}

fn ordinary_to_primitive_string<'js>(object: &Object<'js>) -> Result<Option<Value<'js>>> {
    for key in ["toString", "valueOf"] {
        let method: Value = object.get(key)?;
        let Some(func) = method.as_function() else {
            continue;
        };
        let result: Value = func.call((This(object.clone()),))?;
        if !result.is_object() {
            return Ok(Some(result));
        }
    }
    Ok(None)
}

fn primitive_to_string<'js>(ctx: &Ctx<'js>, value: &Value<'js>) -> Result<String> {
    if value.is_symbol() {
        return Err(Exception::throw_type(
            ctx,
            "cannot convert Symbol to a string",
        ));
    }
    js_to_string(ctx, value)
}
