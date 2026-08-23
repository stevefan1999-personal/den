use std::str::FromStr;

use rquickjs::{
    Class, Ctx, Exception, JsLifetime, Object, Result, Value, atom::PredefinedAtom, class::Trace,
    prelude::Opt,
};
use temporal_rs::{
    Calendar, MonthCode, TimeZone, UtcOffset,
    fields::ZonedDateTimeFields,
    options::{
        DifferenceSettings, Disambiguation, DisplayCalendar, DisplayOffset, DisplayTimeZone,
        OffsetDisambiguation, Overflow, RoundingIncrement, RoundingOptions,
        ToStringRoundingOptions,
    },
    parsed_intermediates::ParsedZonedDateTime,
    parsers::Precision,
    partial::{PartialDuration, PartialTime, PartialZonedDateTime},
    provider::TransitionDirection,
};

use crate::{
    convert::{
        i128_to_bigint, js_to_string, ordering_i32, probe_class, reject_illformed_month_code,
        throw_value_of, to_big_int_i128, to_calendar, to_duration, to_integer_if_integral,
        to_integer_if_integral_i64, to_integer_with_truncation, to_number, to_time_zone,
        to_zoned_date_time, unwrap_temporal,
    },
    duration::Duration,
    instant::Instant,
    plain_date::PlainDate,
    plain_date_time::PlainDateTime,
    plain_month_day::PlainMonthDay,
    plain_time::PlainTime,
    plain_year_month::PlainYearMonth,
};

#[derive(Trace, JsLifetime, Clone)]
#[rquickjs::class(rename = "ZonedDateTime", frozen)]
pub struct ZonedDateTime {
    #[qjs(skip_trace)]
    pub(crate) inner: temporal_rs::ZonedDateTime,
}

impl ZonedDateTime {
    pub(crate) fn wrap(inner: temporal_rs::ZonedDateTime) -> Self {
        Self { inner }
    }
}

#[rquickjs::methods(rename_all = "camelCase")]
impl ZonedDateTime {
    #[qjs(constructor)]
    pub fn new<'js>(
        epoch_nanoseconds: Value<'js>, time_zone: Value<'js>, calendar: Opt<Value<'js>>,
        ctx: Ctx<'js>,
    ) -> Result<Self> {
        let nanoseconds = to_big_int_i128(&ctx, &epoch_nanoseconds)?;
        let zone = time_zone_identifier(&ctx, &time_zone)?;
        let calendar = match calendar.0 {
            None => Calendar::ISO,
            Some(value) if value.is_undefined() => Calendar::ISO,
            Some(value) => calendar_identifier(&ctx, &value)?,
        };
        unwrap_temporal(
            &ctx,
            temporal_rs::ZonedDateTime::try_new(nanoseconds, zone, calendar),
        )
        .map(Self::wrap)
    }

    #[qjs(static)]
    pub fn from<'js>(item: Value<'js>, options: Opt<Value<'js>>, ctx: Ctx<'js>) -> Result<Self> {
        to_zoned(&ctx, &item, options).map(Self::wrap)
    }

    #[qjs(static)]
    pub fn compare<'js>(one: Value<'js>, two: Value<'js>, ctx: Ctx<'js>) -> Result<i32> {
        let left = to_zoned(&ctx, &one, Opt(None))?;
        let right = to_zoned(&ctx, &two, Opt(None))?;
        Ok(ordering_i32(left.compare_instant(&right)))
    }

    #[qjs(get)]
    pub fn calendar_id(&self) -> &'static str {
        self.inner.calendar().identifier()
    }

    #[qjs(get)]
    pub fn time_zone_id(&self, ctx: Ctx<'_>) -> Result<String> {
        unwrap_temporal(&ctx, self.inner.time_zone().identifier())
    }

    #[qjs(get)]
    pub fn epoch_nanoseconds<'js>(&self, ctx: Ctx<'js>) -> Result<rquickjs::BigInt<'js>> {
        i128_to_bigint(ctx, self.inner.epoch_nanoseconds().as_i128())
    }

    #[qjs(get)]
    pub fn epoch_milliseconds(&self) -> i64 {
        self.inner.epoch_milliseconds()
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

    #[qjs(get)]
    pub fn era(&self) -> Option<String> {
        self.inner.era().map(|era| era.to_string())
    }

    #[qjs(get)]
    pub fn era_year(&self) -> Option<i32> {
        self.inner.era_year()
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
    pub fn offset(&self) -> String {
        self.inner.offset()
    }

    #[qjs(get)]
    pub fn offset_nanoseconds(&self) -> i64 {
        self.inner.offset_nanoseconds()
    }

    #[qjs(get)]
    pub fn hours_in_day(&self, ctx: Ctx<'_>) -> Result<f64> {
        unwrap_temporal(&ctx, self.inner.hours_in_day())
    }

    pub fn add<'js>(
        &self, duration_like: Value<'js>, options: Opt<Value<'js>>, ctx: Ctx<'js>,
    ) -> Result<Self> {
        let duration = duration_like_value(&ctx, &duration_like)?;
        let overflow = overflow_option(&ctx, options)?;
        unwrap_temporal(&ctx, self.inner.add(&duration, overflow)).map(Self::wrap)
    }

    pub fn subtract<'js>(
        &self, duration_like: Value<'js>, options: Opt<Value<'js>>, ctx: Ctx<'js>,
    ) -> Result<Self> {
        let duration = duration_like_value(&ctx, &duration_like)?;
        let overflow = overflow_option(&ctx, options)?;
        unwrap_temporal(&ctx, self.inner.subtract(&duration, overflow)).map(Self::wrap)
    }

    pub fn until<'js>(
        &self, other: Value<'js>, options: Opt<Value<'js>>, ctx: Ctx<'js>,
    ) -> Result<Duration> {
        let other = to_zoned(&ctx, &other, Opt(None))?;
        let settings = difference_settings(&ctx, options)?;
        unwrap_temporal(&ctx, self.inner.until(&other, settings)).map(Duration::wrap)
    }

    pub fn since<'js>(
        &self, other: Value<'js>, options: Opt<Value<'js>>, ctx: Ctx<'js>,
    ) -> Result<Duration> {
        let other = to_zoned(&ctx, &other, Opt(None))?;
        let settings = difference_settings(&ctx, options)?;
        unwrap_temporal(&ctx, self.inner.since(&other, settings)).map(Duration::wrap)
    }

    pub fn equals<'js>(&self, other: Value<'js>, ctx: Ctx<'js>) -> Result<bool> {
        let other = to_zoned(&ctx, &other, Opt(None))?;
        unwrap_temporal(&ctx, self.inner.equals(&other))
    }

    pub fn with<'js>(
        &self, temporal_zoned_date_time_like: Value<'js>, options: Opt<Value<'js>>, ctx: Ctx<'js>,
    ) -> Result<Self> {
        let object = temporal_zoned_date_time_like
            .as_object()
            .ok_or_else(|| Exception::throw_type(&ctx, "with() argument must be an object"))?;
        reject_calendar_or_time_zone(&ctx, &temporal_zoned_date_time_like, object)?;
        let (fields, month_code) = zoned_fields_from_object(&ctx, object)?;
        let fields = apply_year(&ctx, object, fields)?;
        let (disambiguation, offset_option, overflow) =
            zoned_options(&ctx, options, OffsetDisambiguation::Prefer)?;
        let fields = apply_zoned_month_code(&ctx, fields, month_code)?;
        unwrap_temporal(
            &ctx,
            self.inner
                .with(fields, disambiguation, offset_option, overflow),
        )
        .map(Self::wrap)
    }

    pub fn with_calendar<'js>(&self, calendar: Value<'js>, ctx: Ctx<'js>) -> Result<Self> {
        if calendar.is_undefined() {
            return Err(Exception::throw_type(&ctx, "calendar is required"));
        }
        Ok(Self::wrap(
            self.inner
                .with_calendar(calendar_from_value(&ctx, &calendar)?),
        ))
    }

    pub fn with_time_zone<'js>(&self, time_zone: Value<'js>, ctx: Ctx<'js>) -> Result<Self> {
        let zone = time_zone_from_value(&ctx, &time_zone)?;
        unwrap_temporal(&ctx, self.inner.with_timezone(zone)).map(Self::wrap)
    }

    pub fn with_plain_time<'js>(&self, time: Opt<Value<'js>>, ctx: Ctx<'js>) -> Result<Self> {
        let time = match time.0 {
            None => None,
            Some(value) if value.is_undefined() => None,
            Some(value) => Some(plain_time_from_value(&ctx, &value)?),
        };
        unwrap_temporal(&ctx, self.inner.with_plain_time(time)).map(Self::wrap)
    }

    pub fn to_instant(&self) -> Instant {
        Instant::wrap(self.inner.to_instant())
    }

    pub fn to_plain_date(&self) -> PlainDate {
        PlainDate::wrap(self.inner.to_plain_date())
    }

    pub fn to_plain_time(&self) -> PlainTime {
        PlainTime::wrap(self.inner.to_plain_time())
    }

    pub fn to_plain_date_time(&self) -> PlainDateTime {
        PlainDateTime::wrap(self.inner.to_plain_date_time())
    }

    pub fn start_of_day(&self, ctx: Ctx<'_>) -> Result<Self> {
        unwrap_temporal(&ctx, self.inner.start_of_day()).map(Self::wrap)
    }

    pub fn get_time_zone_transition<'js>(
        &self, direction_param: Value<'js>, ctx: Ctx<'js>,
    ) -> Result<Value<'js>> {
        let direction = direction_option(&ctx, &direction_param)?;
        match unwrap_temporal(&ctx, self.inner.get_time_zone_transition(direction))? {
            Some(inner) => Ok(Class::instance(ctx.clone(), Self::wrap(inner))?.into_value()),
            None => Ok(Value::new_null(ctx)),
        }
    }

    pub fn round<'js>(&self, options: Value<'js>, ctx: Ctx<'js>) -> Result<Self> {
        if options.is_undefined() {
            return Err(Exception::throw_type(&ctx, "smallestUnit is required"));
        }
        let rounding = if options.is_string() {
            let mut rounding = RoundingOptions::default();
            rounding.smallest_unit = Some(unit_from_value(&ctx, &options)?);
            rounding
        } else {
            let object = options.as_object().ok_or_else(|| {
                Exception::throw_type(&ctx, "round options must be an object or string")
            })?;
            datetime_rounding_options(&ctx, object)?
        };
        unwrap_temporal(&ctx, self.inner.round(rounding)).map(Self::wrap)
    }

    pub fn to_string<'js>(&self, options: Opt<Value<'js>>, ctx: Ctx<'js>) -> Result<String> {
        let (display_offset, display_timezone, display_calendar, rounding) =
            to_string_options(&ctx, options)?;
        unwrap_temporal(
            &ctx,
            self.inner.to_ixdtf_string(
                display_offset,
                display_timezone,
                display_calendar,
                rounding,
            ),
        )
    }

    #[qjs(rename = "toJSON")]
    pub fn to_json(&self, ctx: Ctx<'_>) -> Result<String> {
        unwrap_temporal(
            &ctx,
            self.inner.to_ixdtf_string(
                DisplayOffset::Auto,
                DisplayTimeZone::Auto,
                DisplayCalendar::Auto,
                ToStringRoundingOptions::default(),
            ),
        )
    }

    pub fn to_locale_string(&self, ctx: Ctx<'_>) -> Result<String> {
        self.to_json(ctx)
    }

    pub fn value_of(&self, ctx: Ctx<'_>) -> Result<()> {
        Err(throw_value_of(&ctx, "Temporal.ZonedDateTime"))
    }

    #[qjs(prop, rename = PredefinedAtom::SymbolToStringTag, configurable)]
    pub fn to_string_tag() -> &'static str {
        "Temporal.ZonedDateTime"
    }
}

fn get_optional<'js>(object: &Object<'js>, key: &str) -> Result<Option<Value<'js>>> {
    let value: Value<'js> = object.get(key)?;
    if value.is_undefined() {
        Ok(None)
    } else {
        Ok(Some(value))
    }
}

fn options_object<'js>(ctx: &Ctx<'js>, options: Opt<Value<'js>>) -> Result<Option<Object<'js>>> {
    let Some(value) = options.0 else {
        return Ok(None);
    };
    if value.is_undefined() {
        return Ok(None);
    }
    value
        .as_object()
        .cloned()
        .ok_or_else(|| Exception::throw_type(ctx, "options must be an object"))
        .map(Some)
}

fn truncated_i32<'js>(ctx: &Ctx<'js>, value: &Value<'js>) -> Result<i32> {
    i32::try_from(to_integer_with_truncation(ctx, value)?)
        .map_err(|_| Exception::throw_range(ctx, "integer is out of range"))
}

fn truncated_u8<'js>(ctx: &Ctx<'js>, value: &Value<'js>) -> Result<u8> {
    u8::try_from(truncated_i32(ctx, value)?)
        .map_err(|_| Exception::throw_range(ctx, "integer is out of range"))
}

fn truncated_u16<'js>(ctx: &Ctx<'js>, value: &Value<'js>) -> Result<u16> {
    u16::try_from(truncated_i32(ctx, value)?)
        .map_err(|_| Exception::throw_range(ctx, "integer is out of range"))
}

fn time_zone_identifier<'js>(ctx: &Ctx<'js>, value: &Value<'js>) -> Result<TimeZone> {
    if !value.is_string() {
        return Err(Exception::throw_type(
            ctx,
            "time zone must be a time zone identifier string",
        ));
    }
    let identifier = value.get::<String>()?;
    unwrap_temporal(ctx, TimeZone::try_from_identifier_str(&identifier))
}

fn calendar_identifier<'js>(ctx: &Ctx<'js>, value: &Value<'js>) -> Result<Calendar> {
    if !value.is_string() {
        return Err(Exception::throw_type(
            ctx,
            "calendar must be a calendar identifier string",
        ));
    }
    let identifier = value.get::<String>()?;
    unwrap_temporal(ctx, Calendar::try_from_utf8(identifier.as_bytes()))
}

fn calendar_from_value<'js>(ctx: &Ctx<'js>, value: &Value<'js>) -> Result<Calendar> {
    if let Some(zoned) = probe_class::<ZonedDateTime>(ctx, value) {
        return Ok(zoned.inner.calendar().clone());
    }
    if let Some(date) = probe_class::<PlainDate>(ctx, value) {
        return Ok(date.inner.calendar().clone());
    }
    if let Some(date_time) = probe_class::<PlainDateTime>(ctx, value) {
        return Ok(date_time.inner.calendar().clone());
    }
    if let Some(year_month) = probe_class::<PlainYearMonth>(ctx, value) {
        return Ok(year_month.inner.calendar().clone());
    }
    if let Some(month_day) = probe_class::<PlainMonthDay>(ctx, value) {
        return Ok(month_day.inner.calendar().clone());
    }
    to_calendar(ctx, value)
}

fn to_option_string<'js>(ctx: &Ctx<'js>, value: &Value<'js>) -> Result<String> {
    if value.is_symbol() {
        return Err(Exception::throw_type(
            ctx,
            "cannot convert Symbol to a string",
        ));
    }
    if value.is_null() {
        return Ok("null".to_string());
    }
    if let Some(flag) = value.as_bool() {
        return Ok(if flag { "true" } else { "false" }.to_string());
    }
    if let Some(number) = value.as_number() {
        return Ok(number.to_string());
    }
    if value.is_big_int() {
        return js_to_string(ctx, value);
    }
    js_to_string(ctx, value)
}

fn overflow_from_value<'js>(ctx: &Ctx<'js>, value: &Value<'js>) -> Result<Overflow> {
    Overflow::from_str(&to_option_string(ctx, value)?)
        .map_err(|_| Exception::throw_range(ctx, "invalid overflow option"))
}

fn unit_from_value<'js>(ctx: &Ctx<'js>, value: &Value<'js>) -> Result<temporal_rs::options::Unit> {
    temporal_rs::options::Unit::from_str(&to_option_string(ctx, value)?)
        .map_err(|_| Exception::throw_range(ctx, "invalid Temporal unit"))
}

fn rounding_mode_from_value<'js>(
    ctx: &Ctx<'js>, value: &Value<'js>,
) -> Result<temporal_rs::options::RoundingMode> {
    temporal_rs::options::RoundingMode::from_str(&to_option_string(ctx, value)?)
        .map_err(|_| Exception::throw_range(ctx, "invalid roundingMode"))
}

fn display_calendar_from_value<'js>(ctx: &Ctx<'js>, value: &Value<'js>) -> Result<DisplayCalendar> {
    DisplayCalendar::from_str(&to_option_string(ctx, value)?)
        .map_err(|_| Exception::throw_range(ctx, "invalid calendarName option"))
}

fn fractional_second_digits<'js>(ctx: &Ctx<'js>, value: &Value<'js>) -> Result<Precision> {
    if value.is_number() || value.as_int().is_some() || value.as_float().is_some() {
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

fn require_string_field<'js>(ctx: &Ctx<'js>, value: &Value<'js>) -> Result<String> {
    if let Some(string) = value.as_string() {
        return string.to_string();
    }
    let Some(object) = value.as_object() else {
        return Err(Exception::throw_type(ctx, "must be a string"));
    };
    let to_string: rquickjs::Function = object.get(PredefinedAtom::ToString)?;
    let result: Value = to_string.call((rquickjs::function::This(object.clone()),))?;
    result
        .as_string()
        .ok_or_else(|| Exception::throw_type(ctx, "must be a string"))?
        .to_string()
}

fn parse_offset_string<'js>(ctx: &Ctx<'js>, value: &Value<'js>) -> Result<UtcOffset> {
    let offset = if value.is_string() {
        value.get::<String>()?
    } else if value.is_object() {
        to_option_string(ctx, value)?
    } else {
        return Err(Exception::throw_type(ctx, "offset must be a string"));
    };
    unwrap_temporal(ctx, UtcOffset::from_utf8(offset.as_bytes()))
}

fn time_zone_from_value<'js>(ctx: &Ctx<'js>, value: &Value<'js>) -> Result<TimeZone> {
    if let Some(zoned) = probe_class::<ZonedDateTime>(ctx, value) {
        return Ok(*zoned.inner.time_zone());
    }
    to_time_zone(ctx, value)
}

fn reject_calendar_or_time_zone<'js>(
    ctx: &Ctx<'js>, value: &Value<'js>, object: &Object<'js>,
) -> Result<()> {
    if probe_class::<ZonedDateTime>(ctx, value).is_some()
        || probe_class::<PlainDate>(ctx, value).is_some()
        || probe_class::<PlainDateTime>(ctx, value).is_some()
        || probe_class::<PlainTime>(ctx, value).is_some()
        || probe_class::<PlainYearMonth>(ctx, value).is_some()
        || probe_class::<PlainMonthDay>(ctx, value).is_some()
    {
        return Err(Exception::throw_type(
            ctx,
            "calendar and timeZone are not allowed in with()",
        ));
    }
    if get_optional(object, "calendar")?.is_some() || get_optional(object, "timeZone")?.is_some() {
        return Err(Exception::throw_type(
            ctx,
            "calendar and timeZone are not allowed in with()",
        ));
    }
    Ok(())
}

fn zoned_fields_from_object<'js>(
    ctx: &Ctx<'js>, object: &Object<'js>,
) -> Result<(ZonedDateTimeFields, Option<String>)> {
    let mut fields = ZonedDateTimeFields::new();
    if let Some(value) = get_optional(object, "day")? {
        fields.calendar_fields = fields.calendar_fields.with_day(truncated_u8(ctx, &value)?);
    }
    if let Some(value) = get_optional(object, "hour")? {
        fields.time.hour = Some(truncated_u8(ctx, &value)?);
    }
    if let Some(value) = get_optional(object, "microsecond")? {
        fields.time.microsecond = Some(truncated_u16(ctx, &value)?);
    }
    if let Some(value) = get_optional(object, "millisecond")? {
        fields.time.millisecond = Some(truncated_u16(ctx, &value)?);
    }
    if let Some(value) = get_optional(object, "minute")? {
        fields.time.minute = Some(truncated_u8(ctx, &value)?);
    }
    if let Some(value) = get_optional(object, "month")? {
        fields.calendar_fields = fields
            .calendar_fields
            .with_month(truncated_u8(ctx, &value)?);
    }
    let month_code = if let Some(value) = get_optional(object, "monthCode")? {
        let code = require_string_field(ctx, &value)?;
        reject_illformed_month_code(ctx, &code)?;
        Some(code)
    } else {
        None
    };
    if let Some(value) = get_optional(object, "nanosecond")? {
        fields.time.nanosecond = Some(truncated_u16(ctx, &value)?);
    }
    if let Some(value) = get_optional(object, "offset")? {
        fields.offset = Some(parse_offset_string(ctx, &value)?);
    }
    if let Some(value) = get_optional(object, "second")? {
        fields.time.second = Some(truncated_u8(ctx, &value)?);
    }
    Ok((fields, month_code))
}

fn apply_zoned_month_code<'js>(
    ctx: &Ctx<'js>, mut fields: ZonedDateTimeFields, month_code: Option<String>,
) -> Result<ZonedDateTimeFields> {
    if let Some(code) = month_code {
        let month_code = unwrap_temporal(ctx, MonthCode::try_from_utf8(code.as_bytes()))?;
        fields.calendar_fields = fields.calendar_fields.with_month_code(month_code);
    }
    Ok(fields)
}

fn apply_year<'js>(
    ctx: &Ctx<'js>, object: &Object<'js>, mut fields: ZonedDateTimeFields,
) -> Result<ZonedDateTimeFields> {
    if let Some(value) = get_optional(object, "year")? {
        fields.calendar_fields = fields
            .calendar_fields
            .with_year(truncated_i32(ctx, &value)?);
    }
    Ok(fields)
}

fn zoned_options<'js>(
    ctx: &Ctx<'js>, options: Opt<Value<'js>>, default_offset: OffsetDisambiguation,
) -> Result<(
    Option<Disambiguation>,
    Option<OffsetDisambiguation>,
    Option<Overflow>,
)> {
    let Some(object) = options_object(ctx, options)? else {
        return Ok((None, Some(default_offset), None));
    };
    let disambiguation = match get_optional(&object, "disambiguation")? {
        None => None,
        Some(value) => {
            let name = to_option_string(ctx, &value)?;
            Some(
                Disambiguation::from_str(&name)
                    .map_err(|_| Exception::throw_range(ctx, "invalid disambiguation"))?,
            )
        }
    };
    let offset_option = match get_optional(&object, "offset")? {
        None => Some(default_offset),
        Some(value) => {
            let name = to_option_string(ctx, &value)?;
            Some(
                OffsetDisambiguation::from_str(&name)
                    .map_err(|_| Exception::throw_range(ctx, "invalid offset option"))?,
            )
        }
    };
    let overflow = match get_optional(&object, "overflow")? {
        None => None,
        Some(value) => Some(overflow_from_value(ctx, &value)?),
    };
    Ok((disambiguation, offset_option, overflow))
}

fn to_zoned<'js>(
    ctx: &Ctx<'js>, value: &Value<'js>, options: Opt<Value<'js>>,
) -> Result<temporal_rs::ZonedDateTime> {
    if let Some(zoned) = probe_class::<ZonedDateTime>(ctx, value) {
        if options.0.is_none() {
            return to_zoned_date_time(ctx, value);
        }
        let _ = zoned_options(ctx, options, OffsetDisambiguation::Reject)?;
        return Ok(zoned.inner);
    }
    if let Some(object) = value.as_object() {
        let calendar = match get_optional(object, "calendar")? {
            None => Calendar::ISO,
            Some(calendar) => calendar_from_value(ctx, &calendar)?,
        };
        let (fields, month_code) = zoned_fields_from_object(ctx, object)?;
        let timezone = match get_optional(object, "timeZone")? {
            None => {
                return Err(Exception::throw_type(ctx, "timeZone is required"));
            }
            Some(zone) => time_zone_from_value(ctx, &zone)?,
        };
        let fields = apply_year(ctx, object, fields)?;
        let (disambiguation, offset_option, overflow) =
            zoned_options(ctx, options, OffsetDisambiguation::Reject)?;
        let fields = apply_zoned_month_code(ctx, fields, month_code)?;
        let partial = PartialZonedDateTime {
            fields,
            timezone: Some(timezone),
            calendar,
        };
        return unwrap_temporal(
            ctx,
            temporal_rs::ZonedDateTime::from_partial(
                partial,
                overflow,
                disambiguation,
                offset_option,
            ),
        );
    }
    if value.is_string() {
        let string = value.get::<String>()?;
        let parsed = unwrap_temporal(ctx, ParsedZonedDateTime::from_utf8(string.as_bytes()))?;
        let (disambiguation, offset_option, _overflow) =
            zoned_options(ctx, options, OffsetDisambiguation::Reject)?;
        return unwrap_temporal(
            ctx,
            temporal_rs::ZonedDateTime::from_parsed(
                parsed,
                disambiguation.unwrap_or(Disambiguation::Compatible),
                offset_option.unwrap_or(OffsetDisambiguation::Reject),
            ),
        );
    }
    Err(Exception::throw_type(
        ctx,
        "cannot convert value to Temporal.ZonedDateTime",
    ))
}

fn overflow_option<'js>(ctx: &Ctx<'js>, options: Opt<Value<'js>>) -> Result<Option<Overflow>> {
    let Some(object) = options_object(ctx, options)? else {
        return Ok(None);
    };
    match get_optional(&object, "overflow")? {
        None => Ok(None),
        Some(value) => overflow_from_value(ctx, &value).map(Some),
    }
}

fn duration_like_value<'js>(ctx: &Ctx<'js>, value: &Value<'js>) -> Result<temporal_rs::Duration> {
    if probe_class::<Duration>(ctx, value).is_some() || !value.is_object() {
        return to_duration(ctx, value);
    }
    let Some(object) = value.as_object() else {
        return to_duration(ctx, value);
    };
    let partial = PartialDuration {
        days: optional_integral_i64(ctx, object, "days")?,
        hours: optional_integral_i64(ctx, object, "hours")?,
        microseconds: optional_integral_i128(ctx, object, "microseconds")?,
        milliseconds: optional_integral_i64(ctx, object, "milliseconds")?,
        minutes: optional_integral_i64(ctx, object, "minutes")?,
        months: optional_integral_i64(ctx, object, "months")?,
        nanoseconds: optional_integral_i128(ctx, object, "nanoseconds")?,
        seconds: optional_integral_i64(ctx, object, "seconds")?,
        weeks: optional_integral_i64(ctx, object, "weeks")?,
        years: optional_integral_i64(ctx, object, "years")?,
    };
    unwrap_temporal(ctx, temporal_rs::Duration::from_partial_duration(partial))
}

fn optional_integral_i64<'js>(
    ctx: &Ctx<'js>, object: &Object<'js>, key: &str,
) -> Result<Option<i64>> {
    match get_optional(object, key)? {
        None => Ok(None),
        Some(value) => to_integer_if_integral_i64(ctx, &value).map(Some),
    }
}

fn optional_integral_i128<'js>(
    ctx: &Ctx<'js>, object: &Object<'js>, key: &str,
) -> Result<Option<i128>> {
    match get_optional(object, key)? {
        None => Ok(None),
        Some(value) => to_integer_if_integral(ctx, &value).map(Some),
    }
}

fn difference_settings<'js>(
    ctx: &Ctx<'js>, options: Opt<Value<'js>>,
) -> Result<DifferenceSettings> {
    let mut settings = DifferenceSettings::default();
    let Some(object) = options_object(ctx, options)? else {
        return Ok(settings);
    };
    settings.largest_unit = match get_optional(&object, "largestUnit")? {
        None => None,
        Some(value) => Some(unit_from_value(ctx, &value)?),
    };
    settings.increment = match get_optional(&object, "roundingIncrement")? {
        None => None,
        Some(value) => {
            let number = to_number(ctx, &value)?;
            Some(unwrap_temporal(ctx, RoundingIncrement::try_from(number))?)
        }
    };
    settings.rounding_mode = match get_optional(&object, "roundingMode")? {
        None => None,
        Some(value) => Some(rounding_mode_from_value(ctx, &value)?),
    };
    settings.smallest_unit = match get_optional(&object, "smallestUnit")? {
        None => None,
        Some(value) => Some(unit_from_value(ctx, &value)?),
    };
    Ok(settings)
}

fn datetime_rounding_options<'js>(ctx: &Ctx<'js>, object: &Object<'js>) -> Result<RoundingOptions> {
    let mut options = RoundingOptions::default();
    options.increment = match get_optional(object, "roundingIncrement")? {
        None => None,
        Some(value) => {
            let number = to_number(ctx, &value)?;
            Some(unwrap_temporal(ctx, RoundingIncrement::try_from(number))?)
        }
    };
    options.rounding_mode = match get_optional(object, "roundingMode")? {
        None => None,
        Some(value) => Some(rounding_mode_from_value(ctx, &value)?),
    };
    options.smallest_unit = match get_optional(object, "smallestUnit")? {
        None => None,
        Some(value) => Some(unit_from_value(ctx, &value)?),
    };
    Ok(options)
}

fn direction_option<'js>(ctx: &Ctx<'js>, value: &Value<'js>) -> Result<TransitionDirection> {
    let direction = if value.is_string() {
        to_option_string(ctx, value)?
    } else {
        let object = value
            .as_object()
            .ok_or_else(|| Exception::throw_type(ctx, "direction must be a string or object"))?;
        match get_optional(object, "direction")? {
            None => {
                return Err(Exception::throw_range(ctx, "direction is required"));
            }
            Some(direction) => to_option_string(ctx, &direction)?,
        }
    };
    TransitionDirection::from_str(&direction)
        .map_err(|_| Exception::throw_range(ctx, "invalid direction"))
}

fn plain_time_from_value<'js>(
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
    let Some(object) = value.as_object() else {
        return Err(Exception::throw_type(
            ctx,
            "cannot convert value to Temporal.PlainTime",
        ));
    };
    let partial = PartialTime {
        hour: optional_truncated_u8(ctx, object, "hour")?,
        microsecond: optional_truncated_u16(ctx, object, "microsecond")?,
        millisecond: optional_truncated_u16(ctx, object, "millisecond")?,
        minute: optional_truncated_u8(ctx, object, "minute")?,
        nanosecond: optional_truncated_u16(ctx, object, "nanosecond")?,
        second: optional_truncated_u8(ctx, object, "second")?,
    };
    if partial.is_empty() {
        return Err(Exception::throw_type(
            ctx,
            "time property bag must have at least one field",
        ));
    }
    unwrap_temporal(ctx, temporal_rs::PlainTime::from_partial(partial, None))
}

fn optional_truncated_u8<'js>(
    ctx: &Ctx<'js>, object: &Object<'js>, key: &str,
) -> Result<Option<u8>> {
    match get_optional(object, key)? {
        None => Ok(None),
        Some(value) => truncated_u8(ctx, &value).map(Some),
    }
}

fn optional_truncated_u16<'js>(
    ctx: &Ctx<'js>, object: &Object<'js>, key: &str,
) -> Result<Option<u16>> {
    match get_optional(object, key)? {
        None => Ok(None),
        Some(value) => truncated_u16(ctx, &value).map(Some),
    }
}

fn to_string_options<'js>(
    ctx: &Ctx<'js>, options: Opt<Value<'js>>,
) -> Result<(
    DisplayOffset,
    DisplayTimeZone,
    DisplayCalendar,
    ToStringRoundingOptions,
)> {
    let Some(object) = options_object(ctx, options)? else {
        return Ok((
            DisplayOffset::Auto,
            DisplayTimeZone::Auto,
            DisplayCalendar::Auto,
            ToStringRoundingOptions::default(),
        ));
    };
    let display_calendar = match get_optional(&object, "calendarName")? {
        None => DisplayCalendar::Auto,
        Some(value) => display_calendar_from_value(ctx, &value)?,
    };
    let precision = match get_optional(&object, "fractionalSecondDigits")? {
        None => Precision::Auto,
        Some(value) => fractional_second_digits(ctx, &value)?,
    };
    let display_offset = match get_optional(&object, "offset")? {
        None => DisplayOffset::Auto,
        Some(value) => {
            let name = to_option_string(ctx, &value)?;
            DisplayOffset::from_str(&name)
                .map_err(|_| Exception::throw_range(ctx, "invalid offset option"))?
        }
    };
    let rounding_mode = match get_optional(&object, "roundingMode")? {
        None => None,
        Some(value) => Some(rounding_mode_from_value(ctx, &value)?),
    };
    let smallest_unit = match get_optional(&object, "smallestUnit")? {
        None => None,
        Some(value) => Some(unit_from_value(ctx, &value)?),
    };
    let display_timezone = match get_optional(&object, "timeZoneName")? {
        None => DisplayTimeZone::Auto,
        Some(value) => {
            let name = to_option_string(ctx, &value)?;
            DisplayTimeZone::from_str(&name)
                .map_err(|_| Exception::throw_range(ctx, "invalid timeZoneName option"))?
        }
    };
    Ok((
        display_offset,
        display_timezone,
        display_calendar,
        ToStringRoundingOptions {
            precision,
            smallest_unit,
            rounding_mode,
        },
    ))
}
