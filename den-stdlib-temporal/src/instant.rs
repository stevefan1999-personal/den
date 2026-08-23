use std::str::FromStr;

use rquickjs::{
    BigInt, Ctx, Exception, Function, JsLifetime, Object, Result, Value,
    atom::PredefinedAtom,
    class::Trace,
    prelude::{Opt, This},
};
use temporal_rs::{
    TimeZone,
    options::{
        DifferenceSettings, RoundingIncrement, RoundingMode, RoundingOptions,
        ToStringRoundingOptions,
    },
    parsers::Precision,
    partial::PartialDuration,
};

use crate::{
    convert::{
        get_defined, i128_to_bigint, js_to_string, optional_integral_i128,
        optional_integral_i64, ordering_i32, probe_class, require_object, throw_value_of,
        to_big_int_i128, to_instant, to_integer_if_integral, to_number, to_time_zone,
        unwrap_temporal,
    },
    duration::Duration,
    zoned_date_time::ZonedDateTime,
};

#[derive(Trace, JsLifetime, Clone, Copy)]
#[rquickjs::class(rename = "Instant", frozen)]
pub struct Instant {
    #[qjs(skip_trace)]
    pub(crate) inner: temporal_rs::Instant,
}

impl Instant {
    pub(crate) fn wrap(inner: temporal_rs::Instant) -> Self {
        Self { inner }
    }
}

/// GetOption string path: prefer `toString` so observers do not see valueOf first.
fn to_js_string<'js>(ctx: &Ctx<'js>, value: &Value<'js>) -> Result<String> {
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
        if let Ok(func) = object.get::<_, Function>("toString") {
            let result: Value = func.call((This(object.clone()),))?;
            if let Some(string) = result.as_string() {
                return string.to_string();
            }
            return js_to_string(ctx, &result);
        }
    }
    js_to_string(ctx, value)
}

fn instant_unit<'js>(ctx: &Ctx<'js>, value: &Value<'js>) -> Result<temporal_rs::options::Unit> {
    let name = to_js_string(ctx, value)?;
    temporal_rs::options::Unit::from_str(&name)
        .map_err(|_| Exception::throw_range(ctx, "invalid Temporal unit"))
}

fn instant_rounding_mode<'js>(ctx: &Ctx<'js>, value: &Value<'js>) -> Result<RoundingMode> {
    let name = to_js_string(ctx, value)?;
    RoundingMode::from_str(&name).map_err(|_| Exception::throw_range(ctx, "invalid roundingMode"))
}

fn optional_unit<'js>(
    ctx: &Ctx<'js>, object: &Object<'js>, key: &str,
) -> Result<Option<temporal_rs::options::Unit>> {
    match get_defined(object, key)? {
        None => Ok(None),
        Some(value) => instant_unit(ctx, &value).map(Some),
    }
}

fn optional_rounding_mode<'js>(
    ctx: &Ctx<'js>, object: &Object<'js>, key: &str,
) -> Result<Option<RoundingMode>> {
    match get_defined(object, key)? {
        None => Ok(None),
        Some(value) => instant_rounding_mode(ctx, &value).map(Some),
    }
}

fn optional_rounding_increment<'js>(
    ctx: &Ctx<'js>, object: &Object<'js>,
) -> Result<Option<RoundingIncrement>> {
    match get_defined(object, "roundingIncrement")? {
        None => Ok(None),
        Some(value) => {
            let number = to_number(ctx, &value)?;
            unwrap_temporal(ctx, RoundingIncrement::try_from(number)).map(Some)
        }
    }
}

/// `ToTemporalDuration` with Instant.add/subtract Get order:
/// days, hours, microseconds, milliseconds, minutes, months, nanoseconds,
/// seconds, weeks, years.
fn to_instant_duration<'js>(ctx: &Ctx<'js>, value: &Value<'js>) -> Result<temporal_rs::Duration> {
    if let Some(duration) = probe_class::<Duration>(ctx, value) {
        return Ok(duration.inner);
    }
    if value.is_string() {
        let string = value.get::<String>()?;
        return unwrap_temporal(ctx, temporal_rs::Duration::from_utf8(string.as_bytes()));
    }
    let object = require_object(ctx, value, "duration must be an object")?;
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
    if days.is_none()
        && hours.is_none()
        && microseconds.is_none()
        && milliseconds.is_none()
        && minutes.is_none()
        && months.is_none()
        && nanoseconds.is_none()
        && seconds.is_none()
        && weeks.is_none()
        && years.is_none()
    {
        return Err(Exception::throw_type(
            ctx,
            "duration must have at least one field",
        ));
    }
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

/// Instant.since/until: Get largestUnit, roundingIncrement, roundingMode, smallestUnit.
fn instant_difference_settings<'js>(
    ctx: &Ctx<'js>, options: Opt<Value<'js>>,
) -> Result<DifferenceSettings> {
    let Some(value) = options.0.filter(|value| !value.is_undefined()) else {
        return Ok(DifferenceSettings::default());
    };
    let object = require_object(ctx, &value, "options must be an object")?;
    let mut settings = DifferenceSettings::default();
    settings.largest_unit = optional_unit(ctx, &object, "largestUnit")?;
    settings.increment = optional_rounding_increment(ctx, &object)?;
    settings.rounding_mode = optional_rounding_mode(ctx, &object, "roundingMode")?;
    settings.smallest_unit = optional_unit(ctx, &object, "smallestUnit")?;
    Ok(settings)
}

/// Instant.round: undefined is TypeError; a string is `smallestUnit`.
/// Get order is roundingIncrement, roundingMode, smallestUnit.
fn instant_rounding_options<'js>(ctx: &Ctx<'js>, options: &Value<'js>) -> Result<RoundingOptions> {
    if options.is_undefined() {
        return Err(Exception::throw_type(ctx, "options is required"));
    }
    if options.is_string() {
        let mut rounding = RoundingOptions::default();
        rounding.smallest_unit = Some(instant_unit(ctx, options)?);
        return Ok(rounding);
    }
    let object = require_object(ctx, options, "options must be an object")?;
    let increment = optional_rounding_increment(ctx, &object)?;
    let rounding_mode = optional_rounding_mode(ctx, &object, "roundingMode")?;
    let smallest_unit = optional_unit(ctx, &object, "smallestUnit")?;
    let mut rounding = RoundingOptions::default();
    rounding.increment = increment;
    rounding.rounding_mode = rounding_mode;
    rounding.smallest_unit = smallest_unit;
    Ok(rounding)
}

/// GetStringOrNumberOption for `fractionalSecondDigits`: Number uses floor.
fn fractional_second_digits<'js>(ctx: &Ctx<'js>, value: &Value<'js>) -> Result<Precision> {
    if value.is_number() {
        let number = to_number(ctx, value)?;
        if !number.is_finite() {
            return Err(Exception::throw_range(
                ctx,
                "fractionalSecondDigits is not finite",
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
    let name = to_js_string(ctx, value)?;
    if name == "auto" {
        Ok(Precision::Auto)
    } else {
        Err(Exception::throw_range(
            ctx,
            "fractionalSecondDigits must be \"auto\" or 0-9",
        ))
    }
}

/// Instant.toString: Get fractionalSecondDigits, roundingMode, smallestUnit, timeZone.
fn instant_to_string_parts<'js>(
    ctx: &Ctx<'js>, options: Opt<Value<'js>>,
) -> Result<(Option<TimeZone>, ToStringRoundingOptions)> {
    let Some(value) = options.0.filter(|value| !value.is_undefined()) else {
        return Ok((None, ToStringRoundingOptions::default()));
    };
    let object = require_object(ctx, &value, "options must be an object")?;
    let precision = match get_defined(&object, "fractionalSecondDigits")? {
        None => Precision::Auto,
        Some(value) => fractional_second_digits(ctx, &value)?,
    };
    let rounding_mode = optional_rounding_mode(ctx, &object, "roundingMode")?;
    let smallest_unit = optional_unit(ctx, &object, "smallestUnit")?;
    let time_zone = match get_defined(&object, "timeZone")? {
        None => None,
        Some(value) => Some(to_time_zone(ctx, &value)?),
    };
    Ok((
        time_zone,
        ToStringRoundingOptions {
            precision,
            smallest_unit,
            rounding_mode,
        },
    ))
}

#[rquickjs::methods(rename_all = "camelCase")]
impl Instant {
    #[qjs(constructor)]
    pub fn new<'js>(epoch_nanoseconds: Value<'js>, ctx: Ctx<'js>) -> Result<Self> {
        let nanoseconds = to_big_int_i128(&ctx, &epoch_nanoseconds)?;
        unwrap_temporal(&ctx, temporal_rs::Instant::try_new(nanoseconds)).map(Self::wrap)
    }

    #[qjs(static)]
    pub fn from<'js>(item: Value<'js>, ctx: Ctx<'js>) -> Result<Self> {
        to_instant(&ctx, &item).map(Self::wrap)
    }

    #[qjs(static)]
    pub fn from_epoch_nanoseconds<'js>(
        epoch_nanoseconds: Value<'js>, ctx: Ctx<'js>,
    ) -> Result<Self> {
        let nanoseconds = to_big_int_i128(&ctx, &epoch_nanoseconds)?;
        unwrap_temporal(&ctx, temporal_rs::Instant::try_new(nanoseconds)).map(Self::wrap)
    }

    #[qjs(static)]
    pub fn from_epoch_milliseconds<'js>(
        epoch_milliseconds: Opt<Value<'js>>, ctx: Ctx<'js>,
    ) -> Result<Self> {
        // NumberToBigInt after ToNumber. Missing/undefined → NaN → RangeError.
        let Some(epoch_milliseconds) = epoch_milliseconds.0.filter(|value| !value.is_undefined())
        else {
            return Err(Exception::throw_range(
                &ctx,
                "epochMilliseconds must be an integer",
            ));
        };
        let milliseconds = to_integer_if_integral(&ctx, &epoch_milliseconds)?;
        let milliseconds = i64::try_from(milliseconds)
            .map_err(|_| Exception::throw_range(&ctx, "epochMilliseconds is out of range"))?;
        unwrap_temporal(
            &ctx,
            temporal_rs::Instant::from_epoch_milliseconds(milliseconds),
        )
        .map(Self::wrap)
    }

    #[qjs(static)]
    pub fn compare<'js>(one: Value<'js>, two: Value<'js>, ctx: Ctx<'js>) -> Result<i32> {
        let left = to_instant(&ctx, &one)?;
        let right = to_instant(&ctx, &two)?;
        Ok(ordering_i32(left.cmp(&right)))
    }

    #[qjs(get)]
    pub fn epoch_nanoseconds<'js>(&self, ctx: Ctx<'js>) -> Result<BigInt<'js>> {
        i128_to_bigint(ctx, self.inner.as_i128())
    }

    #[qjs(get)]
    pub fn epoch_milliseconds(&self) -> i64 {
        self.inner.epoch_milliseconds()
    }

    pub fn add<'js>(&self, duration_like: Value<'js>, ctx: Ctx<'js>) -> Result<Self> {
        let duration = to_instant_duration(&ctx, &duration_like)?;
        unwrap_temporal(&ctx, self.inner.add(&duration)).map(Self::wrap)
    }

    pub fn subtract<'js>(&self, duration_like: Value<'js>, ctx: Ctx<'js>) -> Result<Self> {
        let duration = to_instant_duration(&ctx, &duration_like)?;
        unwrap_temporal(&ctx, self.inner.subtract(&duration)).map(Self::wrap)
    }

    pub fn until<'js>(
        &self, other: Value<'js>, options: Opt<Value<'js>>, ctx: Ctx<'js>,
    ) -> Result<Duration> {
        let other = to_instant(&ctx, &other)?;
        let settings = instant_difference_settings(&ctx, options)?;
        unwrap_temporal(&ctx, self.inner.until(&other, settings)).map(Duration::wrap)
    }

    pub fn since<'js>(
        &self, other: Value<'js>, options: Opt<Value<'js>>, ctx: Ctx<'js>,
    ) -> Result<Duration> {
        let other = to_instant(&ctx, &other)?;
        let settings = instant_difference_settings(&ctx, options)?;
        unwrap_temporal(&ctx, self.inner.since(&other, settings)).map(Duration::wrap)
    }

    pub fn round<'js>(&self, options: Value<'js>, ctx: Ctx<'js>) -> Result<Self> {
        let rounding = instant_rounding_options(&ctx, &options)?;
        unwrap_temporal(&ctx, self.inner.round(rounding)).map(Self::wrap)
    }

    pub fn equals<'js>(&self, other: Value<'js>, ctx: Ctx<'js>) -> Result<bool> {
        Ok(self.inner == to_instant(&ctx, &other)?)
    }

    #[qjs(rename = "toZonedDateTimeISO")]
    pub fn to_zoned_date_time_iso<'js>(
        &self, time_zone: Value<'js>, ctx: Ctx<'js>,
    ) -> Result<ZonedDateTime> {
        let zone = to_time_zone(&ctx, &time_zone)?;
        unwrap_temporal(&ctx, self.inner.to_zoned_date_time_iso(zone)).map(ZonedDateTime::wrap)
    }

    pub fn to_string<'js>(&self, options: Opt<Value<'js>>, ctx: Ctx<'js>) -> Result<String> {
        let (time_zone, rounding) = instant_to_string_parts(&ctx, options)?;
        unwrap_temporal(&ctx, self.inner.to_ixdtf_string(time_zone, rounding))
    }

    #[qjs(rename = "toJSON")]
    pub fn to_json(&self, ctx: Ctx<'_>) -> Result<String> {
        unwrap_temporal(
            &ctx,
            self.inner
                .to_ixdtf_string(None, ToStringRoundingOptions::default()),
        )
    }

    pub fn to_locale_string<'js>(
        &self, _locales: Opt<Value<'js>>, _options: Opt<Value<'js>>, ctx: Ctx<'js>,
    ) -> Result<String> {
        unwrap_temporal(
            &ctx,
            self.inner
                .to_ixdtf_string(None, ToStringRoundingOptions::default()),
        )
    }

    pub fn value_of(&self, ctx: Ctx<'_>) -> Result<()> {
        Err(throw_value_of(&ctx, "Temporal.Instant"))
    }

    #[qjs(prop, rename = PredefinedAtom::SymbolToStringTag, configurable)]
    pub fn to_string_tag() -> &'static str {
        "Temporal.Instant"
    }
}
