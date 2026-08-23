//! JS value → `temporal_rs` conversions.
//!
//! Options bags go through [`IndexMap`] so a dictionary is a dictionary, not an
//! ad-hoc `Object::set` loop. Slot-bearing Temporal objects are probed without
//! leaving a `TypeError` pending when the value is simply the wrong class.

use std::{cmp::Ordering, str::FromStr};

use den_util::Probe as _;
use indexmap::IndexMap;
use rquickjs::{
    BigInt, Class, Coerced, Ctx, Exception, FromJs, Function, Object, Result, Value,
    atom::PredefinedAtom, class::JsClass, function::This, prelude::Opt,
};
use temporal_rs::{
    Calendar, TimeZone, UtcOffset,
    fields::{CalendarFields, DateTimeFields, YearMonthCalendarFields, ZonedDateTimeFields},
    options::{
        DifferenceSettings, DisplayCalendar, Overflow, RelativeTo, RoundingIncrement, RoundingMode,
        RoundingOptions, ToStringRoundingOptions, Unit,
    },
    parsers::Precision,
    partial::{
        PartialDate, PartialDateTime, PartialDuration, PartialTime, PartialYearMonth,
        PartialZonedDateTime,
    },
};

use crate::{
    duration::Duration, instant::Instant, plain_date::PlainDate, plain_date_time::PlainDateTime,
    plain_month_day::PlainMonthDay, plain_time::PlainTime, plain_year_month::PlainYearMonth,
    zoned_date_time::ZonedDateTime,
};

pub fn throw_temporal<'js>(ctx: &Ctx<'js>, error: temporal_rs::TemporalError) -> rquickjs::Error {
    let message = error.to_string();
    match error.kind() {
        temporal_rs::error::ErrorKind::Type => Exception::throw_type(ctx, &message),
        temporal_rs::error::ErrorKind::Range => Exception::throw_range(ctx, &message),
        temporal_rs::error::ErrorKind::Syntax => Exception::throw_syntax(ctx, &message),
        _ => Exception::throw_internal(ctx, &message),
    }
}

pub fn unwrap_temporal<'js, T>(
    ctx: &Ctx<'js>, result: temporal_rs::TemporalResult<T>,
) -> Result<T> {
    result.map_err(|error| throw_temporal(ctx, error))
}

pub fn throw_value_of<'js>(ctx: &Ctx<'js>, name: &str) -> rquickjs::Error {
    Exception::throw_type(ctx, &format!("cannot convert {name} to a primitive value"))
}

pub fn ordering_i32(ordering: Ordering) -> i32 {
    match ordering {
        Ordering::Less => -1,
        Ordering::Equal => 0,
        Ordering::Greater => 1,
    }
}

/// `Class::from_object` is `JS_GetOpaque2`, which throws when the object is a
/// different class. Callers that mean "is this an Instant?" must swallow that.
pub fn probe_class<'js, C>(ctx: &Ctx<'js>, value: &Value<'js>) -> Option<C>
where
    C: JsClass<'js> + Clone,
{
    let object = value.as_object()?;
    let class = ctx.probe(|| Class::<C>::from_object(object))?;
    class.try_borrow().ok().map(|borrowed| (*borrowed).clone())
}

/// Read a property, mapping `undefined` to `None` — the shared shape of the
/// dictionary getters used on option bags and property bags.
pub fn get_defined<'js>(object: &Object<'js>, key: &str) -> Result<Option<Value<'js>>> {
    let value: Value<'js> = object.get(key)?;
    if value.is_undefined() {
        Ok(None)
    } else {
        Ok(Some(value))
    }
}

/// GetOptionsObject: a missing or `undefined` argument means no options at
/// all; anything else must be an object.
pub fn options_object<'js>(
    ctx: &Ctx<'js>, options: Opt<Value<'js>>,
) -> Result<Option<Object<'js>>> {
    match options.0 {
        None => Ok(None),
        Some(value) if value.is_undefined() => Ok(None),
        Some(value) => require_object(ctx, &value, "options must be an object").map(Some),
    }
}

pub fn require_object<'js>(
    ctx: &Ctx<'js>, value: &Value<'js>, message: &str,
) -> Result<Object<'js>> {
    value
        .as_object()
        .cloned()
        .ok_or_else(|| Exception::throw_type(ctx, message))
}

/// Reject `calendar` / `timeZone` properties on the partial bags `with()`
/// takes. Callers supply the messages their interface reports.
pub fn reject_calendar_or_time_zone<'js>(
    ctx: &Ctx<'js>, object: &Object<'js>, calendar_message: &str, time_zone_message: &str,
) -> Result<()> {
    if get_defined(object, "calendar")?.is_some() {
        return Err(Exception::throw_type(ctx, calendar_message));
    }
    if get_defined(object, "timeZone")?.is_some() {
        return Err(Exception::throw_type(ctx, time_zone_message));
    }
    Ok(())
}

pub fn options_bag<'js>(
    ctx: &Ctx<'js>, options: Opt<Value<'js>>,
) -> Result<IndexMap<String, Value<'js>>> {
    let Some(value) = options.0 else {
        return Ok(IndexMap::new());
    };
    if value.is_undefined() {
        return Ok(IndexMap::new());
    }
    if !value.is_object() {
        return Err(Exception::throw_type(ctx, "options must be an object"));
    }
    IndexMap::from_js(ctx, value)
}

pub fn bag_value<'js, 'a>(
    bag: &'a IndexMap<String, Value<'js>>, key: &str,
) -> Option<&'a Value<'js>> {
    bag.get(key).filter(|value| !value.is_undefined())
}

/// ISO month codes are `M` + two digits + optional `L`. Syntax is checked
/// when the field is read; calendar suitability happens later in `from`.
pub fn reject_illformed_month_code<'js>(ctx: &Ctx<'js>, code: &str) -> Result<()> {
    let bytes = code.as_bytes();
    let well_formed = matches!(
        bytes,
        [b'M', tens, ones] | [b'M', tens, ones, b'L']
            if tens.is_ascii_digit() && ones.is_ascii_digit()
    );
    if well_formed {
        Ok(())
    } else {
        Err(Exception::throw_range(ctx, "monthCode is not well-formed"))
    }
}

pub fn js_to_string<'js>(ctx: &Ctx<'js>, value: &Value<'js>) -> Result<String> {
    if value.is_symbol() {
        return Err(Exception::throw_type(
            ctx,
            "cannot convert a Symbol to a String",
        ));
    }
    if let Some(string) = value.as_string() {
        return string.to_string();
    }
    let string: Function = ctx.globals().get(PredefinedAtom::String)?;
    string.call((value.clone(),))
}

/// GetOption string path: prefer `toString` so observers do not see `valueOf`
/// first. A non-string primitive result still goes through `ToString`.
pub fn to_js_string<'js>(ctx: &Ctx<'js>, value: &Value<'js>) -> Result<String> {
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

pub fn to_number<'js>(ctx: &Ctx<'js>, value: &Value<'js>) -> Result<f64> {
    if value.is_big_int() || value.is_symbol() {
        return Err(Exception::throw_type(
            ctx,
            "cannot convert BigInt or Symbol to a Number",
        ));
    }
    Ok(Coerced::<f64>::from_js(ctx, value.clone())?.0)
}

/// `ToIntegerIfIntegral`: finite and already an integer, else `RangeError`.
pub fn to_integer_if_integral<'js>(ctx: &Ctx<'js>, value: &Value<'js>) -> Result<i128> {
    let number = to_number(ctx, value)?;
    if !number.is_finite() {
        return Err(Exception::throw_range(ctx, "integer is not finite"));
    }
    if number.trunc() != number {
        return Err(Exception::throw_range(ctx, "expected an integer"));
    }
    Ok(number as i128)
}

pub fn to_integer_if_integral_i64<'js>(ctx: &Ctx<'js>, value: &Value<'js>) -> Result<i64> {
    let integer = to_integer_if_integral(ctx, value)?;
    i64::try_from(integer).map_err(|_| Exception::throw_range(ctx, "integer is out of range"))
}

/// `ToIntegerWithTruncation`: finite, then truncate toward zero.
pub fn to_integer_with_truncation<'js>(ctx: &Ctx<'js>, value: &Value<'js>) -> Result<i128> {
    let number = to_number(ctx, value)?;
    if !number.is_finite() {
        return Err(Exception::throw_range(ctx, "integer is not finite"));
    }
    Ok(number.trunc() as i128)
}

pub fn ctor_integer_if_integral<'js>(ctx: &Ctx<'js>, value: Opt<Value<'js>>) -> Result<i64> {
    match value.0 {
        None => Ok(0),
        Some(value) if value.is_undefined() => Ok(0),
        Some(value) => to_integer_if_integral_i64(ctx, &value),
    }
}

pub fn ctor_integer_if_integral_i128<'js>(ctx: &Ctx<'js>, value: Opt<Value<'js>>) -> Result<i128> {
    match value.0 {
        None => Ok(0),
        Some(value) if value.is_undefined() => Ok(0),
        Some(value) => to_integer_if_integral(ctx, &value),
    }
}

pub fn required_i32<'js>(ctx: &Ctx<'js>, value: Opt<Value<'js>>, name: &str) -> Result<i32> {
    let Some(value) = value.0.filter(|value| !value.is_undefined()) else {
        return Err(Exception::throw_type(ctx, &format!("{name} is required")));
    };
    let integer = to_integer_with_truncation(ctx, &value)?;
    i32::try_from(integer).map_err(|_| Exception::throw_range(ctx, "integer is out of range"))
}

pub fn required_u8<'js>(ctx: &Ctx<'js>, value: Opt<Value<'js>>, name: &str) -> Result<u8> {
    let integer = required_i32(ctx, value, name)?;
    u8::try_from(integer).map_err(|_| Exception::throw_range(ctx, "integer is out of range"))
}

/// `ToIntegerWithTruncation`, then a range check reporting
/// `"integer is out of range"` for every out-of-range input.
pub fn truncated_i32<'js>(ctx: &Ctx<'js>, value: &Value<'js>) -> Result<i32> {
    let integer = to_integer_with_truncation(ctx, value)?;
    i32::try_from(integer).map_err(|_| Exception::throw_range(ctx, "integer is out of range"))
}

pub fn truncated_u8<'js>(ctx: &Ctx<'js>, value: &Value<'js>) -> Result<u8> {
    u8::try_from(truncated_i32(ctx, value)?)
        .map_err(|_| Exception::throw_range(ctx, "integer is out of range"))
}

pub fn truncated_u16<'js>(ctx: &Ctx<'js>, value: &Value<'js>) -> Result<u16> {
    u16::try_from(truncated_i32(ctx, value)?)
        .map_err(|_| Exception::throw_range(ctx, "integer is out of range"))
}

/// Constructor arguments: an absent argument is `undefined`, which truncation
/// reads as `NaN` and rejects — required fields of `new PlainDate()` and
/// friends throw rather than default.
pub fn ctor_required_i32<'js>(ctx: &Ctx<'js>, value: Opt<Value<'js>>) -> Result<i32> {
    let value = value.0.unwrap_or_else(|| Value::new_undefined(ctx.clone()));
    truncated_i32(ctx, &value)
}

pub fn ctor_required_u8<'js>(ctx: &Ctx<'js>, value: Opt<Value<'js>>) -> Result<u8> {
    let value = value.0.unwrap_or_else(|| Value::new_undefined(ctx.clone()));
    truncated_u8(ctx, &value)
}

pub fn truncated_u8_or_zero<'js>(ctx: &Ctx<'js>, value: Opt<Value<'js>>) -> Result<u8> {
    match value.0 {
        None => Ok(0),
        Some(value) if value.is_undefined() => Ok(0),
        Some(value) => truncated_u8(ctx, &value),
    }
}

pub fn truncated_u16_or_zero<'js>(ctx: &Ctx<'js>, value: Opt<Value<'js>>) -> Result<u16> {
    match value.0 {
        None => Ok(0),
        Some(value) if value.is_undefined() => Ok(0),
        Some(value) => truncated_u16(ctx, &value),
    }
}

/// `ToIntegerIfIntegral` on a defined property; absent stays `None`.
pub fn optional_integral_i64<'js>(
    ctx: &Ctx<'js>, object: &Object<'js>, key: &str,
) -> Result<Option<i64>> {
    match get_defined(object, key)? {
        None => Ok(None),
        Some(value) => to_integer_if_integral_i64(ctx, &value).map(Some),
    }
}

pub fn optional_integral_i128<'js>(
    ctx: &Ctx<'js>, object: &Object<'js>, key: &str,
) -> Result<Option<i128>> {
    match get_defined(object, key)? {
        None => Ok(None),
        Some(value) => to_integer_if_integral(ctx, &value).map(Some),
    }
}

/// `ToIntegerWithTruncation` on a defined property; absent stays `None`.
pub fn optional_truncated_i32<'js>(
    ctx: &Ctx<'js>, object: &Object<'js>, key: &str,
) -> Result<Option<i32>> {
    match get_defined(object, key)? {
        None => Ok(None),
        Some(value) => truncated_i32(ctx, &value).map(Some),
    }
}

pub fn optional_truncated_i128<'js>(
    ctx: &Ctx<'js>, object: &Object<'js>, key: &str,
) -> Result<Option<i128>> {
    match get_defined(object, key)? {
        None => Ok(None),
        Some(value) => to_integer_with_truncation(ctx, &value).map(Some),
    }
}

pub fn optional_truncated_u8<'js>(
    ctx: &Ctx<'js>, object: &Object<'js>, key: &str,
) -> Result<Option<u8>> {
    match get_defined(object, key)? {
        None => Ok(None),
        Some(value) => truncated_u8(ctx, &value).map(Some),
    }
}

pub fn optional_truncated_u16<'js>(
    ctx: &Ctx<'js>, object: &Object<'js>, key: &str,
) -> Result<Option<u16>> {
    match get_defined(object, key)? {
        None => Ok(None),
        Some(value) => truncated_u16(ctx, &value).map(Some),
    }
}

/// `ToBigInt`, then an `i128`. Instant epoch nanoseconds sit well outside
/// `i64`.
pub fn to_big_int_i128<'js>(ctx: &Ctx<'js>, value: &Value<'js>) -> Result<i128> {
    if value.is_number() {
        return Err(Exception::throw_type(
            ctx,
            "cannot convert a Number to a BigInt",
        ));
    }
    if value.is_undefined() || value.is_null() || value.is_symbol() {
        return Err(Exception::throw_type(ctx, "cannot convert value to BigInt"));
    }
    let big_int = if let Some(big_int) = value.as_big_int() {
        big_int.clone()
    } else {
        let ctor: Function = ctx.globals().get("BigInt")?;
        ctor.call((value.clone(),))?
    };
    bigint_to_i128(ctx, big_int)
}

pub fn bigint_to_i128<'js>(ctx: &Ctx<'js>, big_int: BigInt<'js>) -> Result<i128> {
    let to_string: Function = ctx.eval("(value) => value.toString()")?;
    let digits: String = to_string.call((big_int,))?;
    digits
        .parse::<i128>()
        .map_err(|_| Exception::throw_range(ctx, "BigInt is out of range"))
}

pub fn i128_to_bigint<'js>(ctx: Ctx<'js>, value: i128) -> Result<BigInt<'js>> {
    let ctor: Function = ctx.globals().get("BigInt")?;
    ctor.call((value.to_string(),))
}

pub fn to_unit<'js>(ctx: &Ctx<'js>, value: &Value<'js>) -> Result<Unit> {
    let name = js_to_string(ctx, value)?;
    Unit::from_str(&name).map_err(|_| Exception::throw_range(ctx, "invalid Temporal unit"))
}

pub fn to_rounding_mode<'js>(ctx: &Ctx<'js>, value: &Value<'js>) -> Result<RoundingMode> {
    let name = js_to_string(ctx, value)?;
    RoundingMode::from_str(&name).map_err(|_| Exception::throw_range(ctx, "invalid roundingMode"))
}

pub fn to_overflow<'js>(ctx: &Ctx<'js>, value: &Value<'js>) -> Result<Overflow> {
    let name = js_to_string(ctx, value)?;
    Overflow::from_str(&name).map_err(|_| Exception::throw_range(ctx, "invalid overflow option"))
}

pub fn to_display_calendar<'js>(ctx: &Ctx<'js>, value: &Value<'js>) -> Result<DisplayCalendar> {
    let name = js_to_string(ctx, value)?;
    DisplayCalendar::from_str(&name)
        .map_err(|_| Exception::throw_range(ctx, "invalid calendarName option"))
}

pub fn bag_overflow<'js>(
    ctx: &Ctx<'js>, bag: &IndexMap<String, Value<'js>>,
) -> Result<Option<Overflow>> {
    match bag_value(bag, "overflow") {
        None => Ok(None),
        Some(value) => to_overflow(ctx, value).map(Some),
    }
}

pub fn to_string_rounding_options<'js>(
    ctx: &Ctx<'js>, bag: &IndexMap<String, Value<'js>>,
) -> Result<ToStringRoundingOptions> {
    let precision = match bag_value(bag, "fractionalSecondDigits") {
        None => Precision::Auto,
        Some(value) => fractional_second_digits(
            ctx,
            value,
            "fractionalSecondDigits must be \"auto\" or 0-9",
            js_to_string,
        )?,
    };
    let smallest_unit = match bag_value(bag, "smallestUnit") {
        None => None,
        Some(value) => Some(to_unit(ctx, value)?),
    };
    let rounding_mode = match bag_value(bag, "roundingMode") {
        None => None,
        Some(value) => Some(to_rounding_mode(ctx, value)?),
    };
    Ok(ToStringRoundingOptions {
        precision,
        smallest_unit,
        rounding_mode,
    })
}

pub fn to_difference_settings<'js>(
    ctx: &Ctx<'js>, bag: &IndexMap<String, Value<'js>>,
) -> Result<DifferenceSettings> {
    let mut settings = DifferenceSettings::default();
    settings.largest_unit = match bag_value(bag, "largestUnit") {
        None => None,
        Some(value) => Some(to_unit(ctx, value)?),
    };
    settings.smallest_unit = match bag_value(bag, "smallestUnit") {
        None => None,
        Some(value) => Some(to_unit(ctx, value)?),
    };
    settings.rounding_mode = match bag_value(bag, "roundingMode") {
        None => None,
        Some(value) => Some(to_rounding_mode(ctx, value)?),
    };
    settings.increment = rounding_increment(ctx, bag)?;
    Ok(settings)
}

pub fn to_rounding_options<'js>(
    ctx: &Ctx<'js>, bag: &IndexMap<String, Value<'js>>,
) -> Result<RoundingOptions> {
    let mut options = RoundingOptions::default();
    options.largest_unit = match bag_value(bag, "largestUnit") {
        None => Some(Unit::Auto),
        Some(value) => Some(to_unit(ctx, value)?),
    };
    options.smallest_unit = match bag_value(bag, "smallestUnit") {
        None => None,
        Some(value) => Some(to_unit(ctx, value)?),
    };
    options.rounding_mode = match bag_value(bag, "roundingMode") {
        None => None,
        Some(value) => Some(to_rounding_mode(ctx, value)?),
    };
    options.increment = rounding_increment(ctx, bag)?;
    Ok(options)
}

/// `GetStringOrNumberOption` for `fractionalSecondDigits`: a Number is
/// floored, everything else goes through the caller's options-bag string
/// coercion (`"auto"` or RangeError). Interfaces disagree on the not-finite
/// message, so the caller supplies it.
pub fn fractional_second_digits<'js, ToString>(
    ctx: &Ctx<'js>, value: &Value<'js>, not_finite_message: &str, to_string: ToString,
) -> Result<Precision>
where
    ToString: FnOnce(&Ctx<'js>, &Value<'js>) -> Result<String>,
{
    if value.is_number() {
        let number = to_number(ctx, value)?;
        if !number.is_finite() {
            return Err(Exception::throw_range(ctx, not_finite_message));
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
    let name = to_string(ctx, value)?;
    if name == "auto" {
        return Ok(Precision::Auto);
    }
    Err(Exception::throw_range(
        ctx,
        "fractionalSecondDigits must be \"auto\" or 0-9",
    ))
}

fn rounding_increment<'js>(
    ctx: &Ctx<'js>, bag: &IndexMap<String, Value<'js>>,
) -> Result<Option<RoundingIncrement>> {
    match bag_value(bag, "roundingIncrement") {
        None => Ok(None),
        Some(value) => {
            let number = to_number(ctx, value)?;
            unwrap_temporal(ctx, RoundingIncrement::try_from(number)).map(Some)
        }
    }
}

/// The calendar carried by any Temporal object with a `[[Calendar]]` slot.
/// The class probes are disjoint — a value can only be one of them — so the
/// probe order is not observable.
pub fn calendar_slot<'js>(ctx: &Ctx<'js>, value: &Value<'js>) -> Option<Calendar> {
    if let Some(date) = probe_class::<PlainDate>(ctx, value) {
        return Some(date.inner.calendar().clone());
    }
    if let Some(date_time) = probe_class::<PlainDateTime>(ctx, value) {
        return Some(date_time.inner.calendar().clone());
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

pub fn to_calendar<'js>(ctx: &Ctx<'js>, value: &Value<'js>) -> Result<Calendar> {
    if value.is_undefined() {
        return Ok(Calendar::ISO);
    }
    if value.is_string() {
        let identifier = value.get::<String>()?;
        return Calendar::from_str(&identifier).map_err(|error| throw_temporal(ctx, error));
    }
    if let Some(calendar) = calendar_slot(ctx, value) {
        return Ok(calendar);
    }
    if let Some(object) = value.as_object() {
        if let Ok(identifier) = object.get::<_, Value>("calendarId") {
            if !identifier.is_undefined() && !identifier.is_null() {
                return to_calendar(ctx, &identifier);
            }
        }
    }
    Err(Exception::throw_type(
        ctx,
        "calendar must be a calendar identifier string",
    ))
}

pub fn optional_calendar<'js>(ctx: &Ctx<'js>, value: Opt<Value<'js>>) -> Result<Calendar> {
    match value.0 {
        None => Ok(Calendar::ISO),
        Some(value) if value.is_undefined() => Ok(Calendar::ISO),
        Some(value) => to_calendar(ctx, &value),
    }
}

pub fn to_time_zone<'js>(ctx: &Ctx<'js>, value: &Value<'js>) -> Result<TimeZone> {
    if value.is_string() {
        let identifier = value.get::<String>()?;
        return TimeZone::try_from_str(&identifier).map_err(|error| throw_temporal(ctx, error));
    }
    if let Some(zoned) = probe_class::<ZonedDateTime>(ctx, value) {
        return Ok(*zoned.inner.time_zone());
    }
    if let Some(object) = value.as_object() {
        if let Ok(identifier) = object.get::<_, Value>("timeZoneId") {
            if identifier.is_string() {
                return to_time_zone(ctx, &identifier);
            }
        }
    }
    Err(Exception::throw_type(
        ctx,
        "time zone must be a time zone identifier string",
    ))
}

pub fn optional_time_zone<'js>(ctx: &Ctx<'js>, value: Opt<Value<'js>>) -> Result<Option<TimeZone>> {
    match value.0 {
        None => Ok(None),
        Some(value) if value.is_undefined() => Ok(None),
        Some(value) => to_time_zone(ctx, &value).map(Some),
    }
}

pub fn to_instant<'js>(ctx: &Ctx<'js>, value: &Value<'js>) -> Result<temporal_rs::Instant> {
    if let Some(instant) = probe_class::<Instant>(ctx, value) {
        return Ok(instant.inner);
    }
    if let Some(zoned) = probe_class::<ZonedDateTime>(ctx, value) {
        return Ok(zoned.inner.to_instant());
    }
    if value.is_string() {
        let string = value.get::<String>()?;
        return unwrap_temporal(ctx, temporal_rs::Instant::from_utf8(string.as_bytes()));
    }
    if value.is_object() {
        let string = js_to_string(ctx, value)?;
        return unwrap_temporal(ctx, temporal_rs::Instant::from_utf8(string.as_bytes()));
    }
    Err(Exception::throw_type(
        ctx,
        "cannot convert value to Temporal.Instant",
    ))
}

pub fn to_duration<'js>(ctx: &Ctx<'js>, value: &Value<'js>) -> Result<temporal_rs::Duration> {
    if let Some(duration) = probe_class::<Duration>(ctx, value) {
        return Ok(duration.inner);
    }
    if value.is_string() {
        let string = value.get::<String>()?;
        return unwrap_temporal(ctx, temporal_rs::Duration::from_utf8(string.as_bytes()));
    }
    if value.is_object() {
        let bag = IndexMap::from_js(ctx, value.clone())?;
        return duration_from_bag(ctx, &bag);
    }
    Err(Exception::throw_type(
        ctx,
        "cannot convert value to Temporal.Duration",
    ))
}

pub fn duration_from_bag<'js>(
    ctx: &Ctx<'js>, bag: &IndexMap<String, Value<'js>>,
) -> Result<temporal_rs::Duration> {
    let partial = PartialDuration {
        years: bag_optional_i64(ctx, bag, "years")?,
        months: bag_optional_i64(ctx, bag, "months")?,
        weeks: bag_optional_i64(ctx, bag, "weeks")?,
        days: bag_optional_i64(ctx, bag, "days")?,
        hours: bag_optional_i64(ctx, bag, "hours")?,
        minutes: bag_optional_i64(ctx, bag, "minutes")?,
        seconds: bag_optional_i64(ctx, bag, "seconds")?,
        milliseconds: bag_optional_i64(ctx, bag, "milliseconds")?,
        microseconds: bag_optional_i128(ctx, bag, "microseconds")?,
        nanoseconds: bag_optional_i128(ctx, bag, "nanoseconds")?,
    };
    unwrap_temporal(ctx, temporal_rs::Duration::from_partial_duration(partial))
}

fn bag_optional_i64<'js>(
    ctx: &Ctx<'js>, bag: &IndexMap<String, Value<'js>>, key: &str,
) -> Result<Option<i64>> {
    match bag_value(bag, key) {
        None => Ok(None),
        Some(value) => to_integer_if_integral_i64(ctx, value).map(Some),
    }
}

fn bag_optional_i128<'js>(
    ctx: &Ctx<'js>, bag: &IndexMap<String, Value<'js>>, key: &str,
) -> Result<Option<i128>> {
    match bag_value(bag, key) {
        None => Ok(None),
        Some(value) => to_integer_if_integral(ctx, value).map(Some),
    }
}

fn bag_optional_truncated_i32<'js>(
    ctx: &Ctx<'js>, bag: &IndexMap<String, Value<'js>>, key: &str,
) -> Result<Option<i32>> {
    match bag_value(bag, key) {
        None => Ok(None),
        Some(value) => {
            let integer = to_integer_with_truncation(ctx, value)?;
            i32::try_from(integer)
                .map(Some)
                .map_err(|_| Exception::throw_range(ctx, "integer is out of range"))
        }
    }
}

fn bag_optional_truncated_u8<'js>(
    ctx: &Ctx<'js>, bag: &IndexMap<String, Value<'js>>, key: &str,
) -> Result<Option<u8>> {
    match bag_optional_truncated_i32(ctx, bag, key)? {
        None => Ok(None),
        Some(integer) => u8::try_from(integer)
            .map(Some)
            .map_err(|_| Exception::throw_range(ctx, "integer is out of range")),
    }
}

fn bag_optional_truncated_u16<'js>(
    ctx: &Ctx<'js>, bag: &IndexMap<String, Value<'js>>, key: &str,
) -> Result<Option<u16>> {
    match bag_optional_truncated_i32(ctx, bag, key)? {
        None => Ok(None),
        Some(integer) => u16::try_from(integer)
            .map(Some)
            .map_err(|_| Exception::throw_range(ctx, "integer is out of range")),
    }
}

pub fn calendar_fields_from_bag<'js>(
    ctx: &Ctx<'js>, bag: &IndexMap<String, Value<'js>>,
) -> Result<CalendarFields> {
    let mut fields = CalendarFields::new();
    if let Some(year) = bag_optional_truncated_i32(ctx, bag, "year")? {
        fields = fields.with_year(year);
    }
    if let Some(month) = bag_optional_truncated_u8(ctx, bag, "month")? {
        fields = fields.with_month(month);
    }
    if let Some(day) = bag_optional_truncated_u8(ctx, bag, "day")? {
        fields = fields.with_day(day);
    }
    if let Some(value) = bag_value(bag, "monthCode") {
        let code = js_to_string(ctx, value)?;
        let month_code = temporal_rs::MonthCode::try_from_utf8(code.as_bytes())
            .map_err(|error| throw_temporal(ctx, error))?;
        fields = fields.with_month_code(month_code);
    }
    Ok(fields)
}

pub fn partial_time_from_bag<'js>(
    ctx: &Ctx<'js>, bag: &IndexMap<String, Value<'js>>,
) -> Result<PartialTime> {
    Ok(PartialTime {
        hour: bag_optional_truncated_u8(ctx, bag, "hour")?,
        minute: bag_optional_truncated_u8(ctx, bag, "minute")?,
        second: bag_optional_truncated_u8(ctx, bag, "second")?,
        millisecond: bag_optional_truncated_u16(ctx, bag, "millisecond")?,
        microsecond: bag_optional_truncated_u16(ctx, bag, "microsecond")?,
        nanosecond: bag_optional_truncated_u16(ctx, bag, "nanosecond")?,
    })
}

pub fn bag_calendar<'js>(ctx: &Ctx<'js>, bag: &IndexMap<String, Value<'js>>) -> Result<Calendar> {
    match bag_value(bag, "calendar") {
        None => Ok(Calendar::ISO),
        Some(value) => to_calendar(ctx, value),
    }
}

fn bag_utc_offset<'js>(
    ctx: &Ctx<'js>, bag: &IndexMap<String, Value<'js>>,
) -> Result<Option<UtcOffset>> {
    match bag_value(bag, "offset") {
        None => Ok(None),
        Some(value) => {
            let identifier = js_to_string(ctx, value)?;
            UtcOffset::from_str(&identifier)
                .map(Some)
                .map_err(|error| throw_temporal(ctx, error))
        }
    }
}

fn date_from_bag<'js>(
    ctx: &Ctx<'js>, bag: &IndexMap<String, Value<'js>>, overflow: Option<Overflow>,
) -> Result<temporal_rs::PlainDate> {
    let partial = PartialDate {
        calendar_fields: calendar_fields_from_bag(ctx, bag)?,
        calendar: bag_calendar(ctx, bag)?,
    };
    unwrap_temporal(ctx, temporal_rs::PlainDate::from_partial(partial, overflow))
}

fn date_time_from_bag<'js>(
    ctx: &Ctx<'js>, bag: &IndexMap<String, Value<'js>>, overflow: Option<Overflow>,
) -> Result<temporal_rs::PlainDateTime> {
    let partial = PartialDateTime {
        fields: DateTimeFields {
            calendar_fields: calendar_fields_from_bag(ctx, bag)?,
            time: partial_time_from_bag(ctx, bag)?,
        },
        calendar: bag_calendar(ctx, bag)?,
    };
    unwrap_temporal(
        ctx,
        temporal_rs::PlainDateTime::from_partial(partial, overflow),
    )
}

fn year_month_from_bag<'js>(
    ctx: &Ctx<'js>, bag: &IndexMap<String, Value<'js>>, overflow: Option<Overflow>,
) -> Result<temporal_rs::PlainYearMonth> {
    let partial = PartialYearMonth {
        calendar_fields: YearMonthCalendarFields::from(calendar_fields_from_bag(ctx, bag)?),
        calendar: bag_calendar(ctx, bag)?,
    };
    unwrap_temporal(
        ctx,
        temporal_rs::PlainYearMonth::from_partial(partial, overflow),
    )
}

fn month_day_from_bag<'js>(
    ctx: &Ctx<'js>, bag: &IndexMap<String, Value<'js>>, overflow: Option<Overflow>,
) -> Result<temporal_rs::PlainMonthDay> {
    let partial = PartialDate {
        calendar_fields: calendar_fields_from_bag(ctx, bag)?,
        calendar: bag_calendar(ctx, bag)?,
    };
    unwrap_temporal(
        ctx,
        temporal_rs::PlainMonthDay::from_partial(partial, overflow),
    )
}

fn zoned_from_bag<'js>(
    ctx: &Ctx<'js>, bag: &IndexMap<String, Value<'js>>, overflow: Option<Overflow>,
) -> Result<temporal_rs::ZonedDateTime> {
    let timezone = match bag_value(bag, "timeZone") {
        None => None,
        Some(value) => Some(to_time_zone(ctx, value)?),
    };
    let partial = PartialZonedDateTime {
        fields: ZonedDateTimeFields {
            calendar_fields: calendar_fields_from_bag(ctx, bag)?,
            time: partial_time_from_bag(ctx, bag)?,
            offset: bag_utc_offset(ctx, bag)?,
        },
        timezone,
        calendar: bag_calendar(ctx, bag)?,
    };
    unwrap_temporal(
        ctx,
        temporal_rs::ZonedDateTime::from_partial(partial, overflow, None, None),
    )
}

pub fn to_plain_date<'js>(
    ctx: &Ctx<'js>, value: &Value<'js>, overflow: Option<Overflow>,
) -> Result<temporal_rs::PlainDate> {
    if let Some(date) = probe_class::<PlainDate>(ctx, value) {
        return Ok(date.inner);
    }
    if let Some(date_time) = probe_class::<PlainDateTime>(ctx, value) {
        return Ok(date_time.inner.to_plain_date());
    }
    if let Some(zoned) = probe_class::<ZonedDateTime>(ctx, value) {
        return Ok(zoned.inner.to_plain_date());
    }
    if value.is_string() {
        let string = value.get::<String>()?;
        return unwrap_temporal(ctx, temporal_rs::PlainDate::from_utf8(string.as_bytes()));
    }
    if value.is_object() {
        let bag = IndexMap::from_js(ctx, value.clone())?;
        return date_from_bag(ctx, &bag, overflow);
    }
    Err(Exception::throw_type(
        ctx,
        "cannot convert value to Temporal.PlainDate",
    ))
}

pub fn to_plain_time<'js>(
    ctx: &Ctx<'js>, value: &Value<'js>, overflow: Option<Overflow>,
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
    if value.is_object() {
        let bag = IndexMap::from_js(ctx, value.clone())?;
        let partial = partial_time_from_bag(ctx, &bag)?;
        return unwrap_temporal(ctx, temporal_rs::PlainTime::from_partial(partial, overflow));
    }
    Err(Exception::throw_type(
        ctx,
        "cannot convert value to Temporal.PlainTime",
    ))
}

pub fn to_plain_date_time<'js>(
    ctx: &Ctx<'js>, value: &Value<'js>,
) -> Result<temporal_rs::PlainDateTime> {
    if let Some(date_time) = probe_class::<PlainDateTime>(ctx, value) {
        return Ok(date_time.inner);
    }
    if let Some(zoned) = probe_class::<ZonedDateTime>(ctx, value) {
        return Ok(zoned.inner.to_plain_date_time());
    }
    if value.is_string() {
        let string = value.get::<String>()?;
        return unwrap_temporal(
            ctx,
            temporal_rs::PlainDateTime::from_utf8(string.as_bytes()),
        );
    }
    if value.is_object() {
        let bag = IndexMap::from_js(ctx, value.clone())?;
        return date_time_from_bag(ctx, &bag, None);
    }
    Err(Exception::throw_type(
        ctx,
        "cannot convert value to Temporal.PlainDateTime",
    ))
}

pub fn to_plain_year_month<'js>(
    ctx: &Ctx<'js>, value: &Value<'js>,
) -> Result<temporal_rs::PlainYearMonth> {
    if let Some(year_month) = probe_class::<PlainYearMonth>(ctx, value) {
        return Ok(year_month.inner);
    }
    if let Some(date) = probe_class::<PlainDate>(ctx, value) {
        return unwrap_temporal(ctx, date.inner.to_plain_year_month());
    }
    if let Some(date_time) = probe_class::<PlainDateTime>(ctx, value) {
        return unwrap_temporal(ctx, date_time.inner.to_plain_date().to_plain_year_month());
    }
    if let Some(zoned) = probe_class::<ZonedDateTime>(ctx, value) {
        return unwrap_temporal(ctx, zoned.inner.to_plain_date().to_plain_year_month());
    }
    if value.is_string() {
        let string = value.get::<String>()?;
        return unwrap_temporal(
            ctx,
            temporal_rs::PlainYearMonth::from_utf8(string.as_bytes()),
        );
    }
    if value.is_object() {
        let bag = IndexMap::from_js(ctx, value.clone())?;
        return year_month_from_bag(ctx, &bag, None);
    }
    Err(Exception::throw_type(
        ctx,
        "cannot convert value to Temporal.PlainYearMonth",
    ))
}

pub fn to_plain_month_day<'js>(
    ctx: &Ctx<'js>, value: &Value<'js>,
) -> Result<temporal_rs::PlainMonthDay> {
    if let Some(month_day) = probe_class::<PlainMonthDay>(ctx, value) {
        return Ok(month_day.inner);
    }
    if let Some(date) = probe_class::<PlainDate>(ctx, value) {
        return unwrap_temporal(ctx, date.inner.to_plain_month_day());
    }
    if let Some(date_time) = probe_class::<PlainDateTime>(ctx, value) {
        return unwrap_temporal(ctx, date_time.inner.to_plain_date().to_plain_month_day());
    }
    if let Some(zoned) = probe_class::<ZonedDateTime>(ctx, value) {
        return unwrap_temporal(ctx, zoned.inner.to_plain_date().to_plain_month_day());
    }
    if value.is_string() {
        let string = value.get::<String>()?;
        return unwrap_temporal(
            ctx,
            temporal_rs::PlainMonthDay::from_utf8(string.as_bytes()),
        );
    }
    if value.is_object() {
        let bag = IndexMap::from_js(ctx, value.clone())?;
        return month_day_from_bag(ctx, &bag, None);
    }
    Err(Exception::throw_type(
        ctx,
        "cannot convert value to Temporal.PlainMonthDay",
    ))
}

pub fn to_zoned_date_time<'js>(
    ctx: &Ctx<'js>, value: &Value<'js>,
) -> Result<temporal_rs::ZonedDateTime> {
    if let Some(zoned) = probe_class::<ZonedDateTime>(ctx, value) {
        return Ok(zoned.inner);
    }
    if value.is_string() {
        let string = value.get::<String>()?;
        return unwrap_temporal(
            ctx,
            temporal_rs::ZonedDateTime::from_utf8(
                string.as_bytes(),
                temporal_rs::options::Disambiguation::Compatible,
                temporal_rs::options::OffsetDisambiguation::Reject,
            ),
        );
    }
    if value.is_object() {
        let bag = IndexMap::from_js(ctx, value.clone())?;
        return zoned_from_bag(ctx, &bag, None);
    }
    Err(Exception::throw_type(
        ctx,
        "cannot convert value to Temporal.ZonedDateTime",
    ))
}

pub fn to_relative_to<'js>(ctx: &Ctx<'js>, value: &Value<'js>) -> Result<RelativeTo> {
    if let Some(date) = probe_class::<PlainDate>(ctx, value) {
        return Ok(RelativeTo::PlainDate(date.inner));
    }
    if let Some(zoned) = probe_class::<ZonedDateTime>(ctx, value) {
        return Ok(RelativeTo::ZonedDateTime(zoned.inner));
    }
    if value.is_string() {
        let string = value.get::<String>()?;
        return unwrap_temporal(ctx, RelativeTo::try_from_str(&string));
    }
    if value.is_object() {
        let bag = IndexMap::from_js(ctx, value.clone())?;
        if bag_value(&bag, "timeZone").is_some() {
            return zoned_from_bag(ctx, &bag, None).map(RelativeTo::ZonedDateTime);
        }
        return date_from_bag(ctx, &bag, None).map(RelativeTo::PlainDate);
    }
    Err(Exception::throw_type(
        ctx,
        "relativeTo must be a Temporal date",
    ))
}
