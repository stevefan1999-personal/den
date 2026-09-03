use rquickjs::{
    Ctx, Exception, JsLifetime, Object, Result, Value, atom::PredefinedAtom, class::Trace,
    prelude::Opt,
};
use temporal_rs::{
    options::{Overflow, ToStringRoundingOptions},
    partial::PartialTime,
};

use crate::{
    convert::{
        calendar_slot, difference_settings, get_defined, optional_truncated_i128, options_object,
        overflow_option, probe_class, require_object, rounding_options, throw_value_of,
        to_duration, to_string_rounding, truncated_u8_or_zero, truncated_u16_or_zero,
        unwrap_temporal,
    },
    duration::Duration,
    plain_date_time::PlainDateTime,
    zoned_date_time::ZonedDateTime,
};

#[derive(Trace, JsLifetime, Clone, Copy)]
#[rquickjs::class(rename = "PlainTime", frozen)]
pub struct PlainTime {
    #[qjs(skip_trace)]
    pub(crate) inner: temporal_rs::PlainTime,
}

impl PlainTime {
    pub(crate) const fn wrap(inner: temporal_rs::PlainTime) -> Self { Self { inner } }
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
                truncated_u8_or_zero(&ctx, hour)?,
                truncated_u8_or_zero(&ctx, minute)?,
                truncated_u8_or_zero(&ctx, second)?,
                truncated_u16_or_zero(&ctx, millisecond)?,
                truncated_u16_or_zero(&ctx, microsecond)?,
                truncated_u16_or_zero(&ctx, nanosecond)?,
            ),
        )
        .map(Self::wrap)
    }

    #[qjs(static)]
    pub fn from<'js>(item: Value<'js>, options: Opt<Value<'js>>, ctx: Ctx<'js>) -> Result<Self> {
        if let Some(time) = existing_plain_time(&ctx, &item) {
            let _overflow = overflow_option(&ctx, options)?.unwrap_or_default();
            return Ok(Self::wrap(time));
        }
        if item.is_string() {
            let time = parse_plain_time(&ctx, &item)?;
            let _overflow = overflow_option(&ctx, options)?.unwrap_or_default();
            return Ok(Self::wrap(time));
        }
        let object = require_object(&ctx, &item, "cannot convert value to Temporal.PlainTime")?;
        let record = to_time_record(&ctx, &object)?;
        let overflow = overflow_option(&ctx, options)?.unwrap_or_default();
        time_from_record(&ctx, record, overflow).map(Self::wrap)
    }

    #[qjs(static)]
    pub fn compare<'js>(one: Value<'js>, two: Value<'js>, ctx: Ctx<'js>) -> Result<i32> {
        let left = to_temporal_time(&ctx, &one)?;
        let right = to_temporal_time(&ctx, &two)?;
        Ok(left.cmp(&right) as i32)
    }

    #[qjs(get)]
    pub const fn hour(&self) -> u8 { self.inner.hour() }

    #[qjs(get)]
    pub const fn minute(&self) -> u8 { self.inner.minute() }

    #[qjs(get)]
    pub const fn second(&self) -> u8 { self.inner.second() }

    #[qjs(get)]
    pub const fn millisecond(&self) -> u16 { self.inner.millisecond() }

    #[qjs(get)]
    pub const fn microsecond(&self) -> u16 { self.inner.microsecond() }

    #[qjs(get)]
    pub const fn nanosecond(&self) -> u16 { self.inner.nanosecond() }

    pub fn add<'js>(&self, duration_like: Value<'js>, ctx: Ctx<'js>) -> Result<Self> {
        let duration = to_duration(&ctx, &duration_like)?;
        unwrap_temporal(&ctx, self.inner.add(&duration)).map(Self::wrap)
    }

    pub fn subtract<'js>(&self, duration_like: Value<'js>, ctx: Ctx<'js>) -> Result<Self> {
        let duration = to_duration(&ctx, &duration_like)?;
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
        let rounding = rounding_options(&ctx, &round_to)?;
        unwrap_temporal(&ctx, self.inner.round(rounding)).map(Self::wrap)
    }

    pub fn with<'js>(
        &self, temporal_time_like: Value<'js>, options: Opt<Value<'js>>, ctx: Ctx<'js>,
    ) -> Result<Self> {
        let object = require_object(&ctx, &temporal_time_like, "with() requires a property bag")?;
        reject_calendar_or_time_zone(&ctx, &object)?;
        let record = to_time_record(&ctx, &object)?;
        let overflow = overflow_option(&ctx, options)?.unwrap_or_default();
        let partial = partial_from_record(&ctx, record, overflow)?;
        unwrap_temporal(&ctx, self.inner.with(partial, Some(overflow))).map(Self::wrap)
    }

    pub fn equals<'js>(&self, other: Value<'js>, ctx: Ctx<'js>) -> Result<bool> {
        Ok(self.inner == to_temporal_time(&ctx, &other)?)
    }

    pub fn to_string<'js>(&self, options: Opt<Value<'js>>, ctx: Ctx<'js>) -> Result<String> {
        let rounding = match options_object(&ctx, options)? {
            None => ToStringRoundingOptions::default(),
            Some(object) => {
                to_string_rounding(&ctx, &object, "fractionalSecondDigits must be finite")?
            }
        };
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

    pub fn value_of(&self, ctx: Ctx<'_>) -> Result<()> {
        Err(throw_value_of(&ctx, "Temporal.PlainTime"))
    }

    #[qjs(prop, rename = PredefinedAtom::SymbolToStringTag, configurable)]
    pub const fn to_string_tag() -> &'static str { "Temporal.PlainTime" }
}

#[derive(Clone, Copy, Default, PartialEq, Eq)]
struct TimeRecord {
    hour:        Option<i128>,
    minute:      Option<i128>,
    second:      Option<i128>,
    millisecond: Option<i128>,
    microsecond: Option<i128>,
    nanosecond:  Option<i128>,
}

impl TimeRecord {
    fn is_empty(self) -> bool { self == Self::default() }
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

pub fn to_temporal_time<'js>(ctx: &Ctx<'js>, value: &Value<'js>) -> Result<temporal_rs::PlainTime> {
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

/// `ToTemporalTimeRecord`: Get time units in alphabetical order.
fn to_time_record<'js>(ctx: &Ctx<'js>, object: &Object<'js>) -> Result<TimeRecord> {
    let hour = optional_truncated_i128(ctx, object, "hour")?;
    let microsecond = optional_truncated_i128(ctx, object, "microsecond")?;
    let millisecond = optional_truncated_i128(ctx, object, "millisecond")?;
    let minute = optional_truncated_i128(ctx, object, "minute")?;
    let nanosecond = optional_truncated_i128(ctx, object, "nanosecond")?;
    let second = optional_truncated_i128(ctx, object, "second")?;
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
    if probe_class::<PlainTime>(ctx, &value).is_some() || calendar_slot(ctx, &value).is_some() {
        return Err(Exception::throw_type(
            ctx,
            "calendar or time zone objects are not valid for Temporal.PlainTime.with",
        ));
    }
    if get_defined(object, "calendar")?.is_some() {
        return Err(Exception::throw_type(
            ctx,
            "calendar is not allowed on Temporal.PlainTime.with",
        ));
    }
    if get_defined(object, "timeZone")?.is_some() {
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
    u16::try_from(regulated)
        .map_err(|_error| Exception::throw_range(ctx, "time value out of range"))
}

fn partial_from_record(
    ctx: &Ctx<'_>, record: TimeRecord, overflow: Overflow,
) -> Result<PartialTime> {
    Ok(PartialTime {
        hour:        record
            .hour
            .map(|value| regulate_field(ctx, value, 23, overflow).map(|value| value as u8))
            .transpose()?,
        minute:      record
            .minute
            .map(|value| regulate_field(ctx, value, 59, overflow).map(|value| value as u8))
            .transpose()?,
        second:      record
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
        nanosecond:  record
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
