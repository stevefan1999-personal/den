use std::str::FromStr;

use rquickjs::{
    Ctx, Exception, JsLifetime, Object, Result, Value, atom::PredefinedAtom, class::Trace,
    prelude::Opt,
};
use temporal_rs::{
    Calendar, MonthCode,
    fields::{CalendarFields, YearMonthCalendarFields},
    options::{
        DifferenceSettings, DisplayCalendar, Overflow, RoundingIncrement, RoundingMode, Unit,
    },
    partial::PartialYearMonth,
};

use crate::{
    convert::{
        calendar_slot, ctor_required_i32, ctor_required_u8, get_defined, optional_truncated_i32,
        optional_truncated_i128, options_object, ordering_i32, probe_class,
        reject_calendar_or_time_zone, reject_illformed_month_code, throw_value_of, to_duration,
        to_integer_with_truncation, to_number, truncated_u8, unwrap_temporal,
    },
    duration::Duration,
    instant::Instant,
    plain_date::PlainDate,
    plain_time::PlainTime,
};

#[derive(Trace, JsLifetime, Clone)]
#[rquickjs::class(rename = "PlainYearMonth", frozen)]
pub struct PlainYearMonth {
    #[qjs(skip_trace)]
    pub(crate) inner: temporal_rs::PlainYearMonth,
}

impl PlainYearMonth {
    pub(crate) const fn wrap(inner: temporal_rs::PlainYearMonth) -> Self { Self { inner } }
}

#[rquickjs::methods(rename_all = "camelCase")]
impl PlainYearMonth {
    #[qjs(constructor)]
    pub fn new<'js>(
        iso_year: Opt<Value<'js>>, iso_month: Opt<Value<'js>>, calendar: Opt<Value<'js>>,
        reference_iso_day: Opt<Value<'js>>, ctx: Ctx<'js>,
    ) -> Result<Self> {
        let year = ctor_required_i32(&ctx, iso_year)?;
        let month = ctor_required_u8(&ctx, iso_month)?;
        let calendar = canonicalize_calendar(&ctx, calendar)?;
        let reference_day = match reference_iso_day.0 {
            None => None,
            Some(value) if value.is_undefined() => None,
            Some(value) => Some(truncated_u8(&ctx, &value)?),
        };
        unwrap_temporal(
            &ctx,
            temporal_rs::PlainYearMonth::try_new(year, month, reference_day, calendar),
        )
        .map(Self::wrap)
    }

    #[qjs(static)]
    pub fn from<'js>(item: Value<'js>, options: Opt<Value<'js>>, ctx: Ctx<'js>) -> Result<Self> {
        to_year_month(&ctx, &item, options).map(Self::wrap)
    }

    #[qjs(static)]
    pub fn compare<'js>(one: Value<'js>, two: Value<'js>, ctx: Ctx<'js>) -> Result<i32> {
        let left = to_year_month(&ctx, &one, Opt(None))?;
        let right = to_year_month(&ctx, &two, Opt(None))?;
        Ok(ordering_i32(left.compare_iso(&right)))
    }

    #[qjs(get)]
    pub fn calendar_id(&self) -> &'static str { self.inner.calendar().identifier() }

    #[qjs(get)]
    pub fn year(&self) -> i32 { self.inner.year() }

    #[qjs(get)]
    pub fn month(&self) -> u8 { self.inner.month() }

    #[qjs(get)]
    pub fn month_code(&self) -> String { self.inner.month_code().as_str().to_string() }

    #[qjs(get)]
    pub fn days_in_year(&self) -> u16 { self.inner.days_in_year() }

    #[qjs(get)]
    pub fn days_in_month(&self) -> u16 { self.inner.days_in_month() }

    #[qjs(get)]
    pub fn months_in_year(&self) -> u16 { self.inner.months_in_year() }

    #[qjs(get)]
    pub fn in_leap_year(&self) -> bool { self.inner.in_leap_year() }

    #[qjs(get)]
    pub fn era(&self) -> Option<String> { self.inner.era().map(|era| era.to_string()) }

    #[qjs(get)]
    pub fn era_year(&self) -> Option<i32> { self.inner.era_year() }

    pub fn add<'js>(
        &self, duration_like: Value<'js>, options: Opt<Value<'js>>, ctx: Ctx<'js>,
    ) -> Result<Self> {
        let duration = to_duration(&ctx, &duration_like)?;
        let overflow = overflow_option(&ctx, options)?.unwrap_or_default();
        unwrap_temporal(&ctx, self.inner.add(&duration, overflow)).map(Self::wrap)
    }

    pub fn subtract<'js>(
        &self, duration_like: Value<'js>, options: Opt<Value<'js>>, ctx: Ctx<'js>,
    ) -> Result<Self> {
        let duration = to_duration(&ctx, &duration_like)?;
        let overflow = overflow_option(&ctx, options)?.unwrap_or_default();
        unwrap_temporal(&ctx, self.inner.subtract(&duration, overflow)).map(Self::wrap)
    }

    pub fn until<'js>(
        &self, other: Value<'js>, options: Opt<Value<'js>>, ctx: Ctx<'js>,
    ) -> Result<Duration> {
        let other = to_year_month(&ctx, &other, Opt(None))?;
        let settings = difference_settings(&ctx, options)?;
        unwrap_temporal(&ctx, self.inner.until(&other, settings)).map(Duration::wrap)
    }

    pub fn since<'js>(
        &self, other: Value<'js>, options: Opt<Value<'js>>, ctx: Ctx<'js>,
    ) -> Result<Duration> {
        let other = to_year_month(&ctx, &other, Opt(None))?;
        let settings = difference_settings(&ctx, options)?;
        unwrap_temporal(&ctx, self.inner.since(&other, settings)).map(Duration::wrap)
    }

    pub fn with<'js>(
        &self, year_month_like: Value<'js>, options: Opt<Value<'js>>, ctx: Ctx<'js>,
    ) -> Result<Self> {
        if is_temporal_object(&ctx, &year_month_like) {
            return Err(Exception::throw_type(
                &ctx,
                "Temporal.PlainYearMonth.prototype.with requires a partial property bag",
            ));
        }
        let Some(object) = year_month_like.as_object() else {
            return Err(Exception::throw_type(
                &ctx,
                "Temporal.PlainYearMonth.prototype.with requires a property bag",
            ));
        };
        reject_calendar_or_time_zone(
            &ctx,
            object,
            "calendar is not allowed in Temporal.PlainYearMonth.prototype.with",
            "timeZone is not allowed in Temporal.PlainYearMonth.prototype.with",
        )?;
        let raw = year_month_fields(&ctx, object)?;
        if raw.is_empty() {
            return Err(Exception::throw_type(
                &ctx,
                "Temporal.PlainYearMonth.prototype.with requires at least one calendar field",
            ));
        }
        let overflow = overflow_option(&ctx, options)?;
        let fields = raw.into_fields(&ctx, overflow.unwrap_or_default())?;
        unwrap_temporal(&ctx, self.inner.with(fields, overflow)).map(Self::wrap)
    }

    pub fn equals<'js>(&self, other: Value<'js>, ctx: Ctx<'js>) -> Result<bool> {
        Ok(self.inner == to_year_month(&ctx, &other, Opt(None))?)
    }

    pub fn to_plain_date<'js>(&self, item: Value<'js>, ctx: Ctx<'js>) -> Result<PlainDate> {
        let Some(object) = item.as_object() else {
            return Err(Exception::throw_type(
                &ctx,
                "Temporal.PlainYearMonth.prototype.toPlainDate requires a property bag",
            ));
        };
        let day = object.get::<_, Value>("day")?;
        if day.is_undefined() {
            return Err(Exception::throw_type(&ctx, "day is required"));
        }
        let day = constrain_u8(&ctx, to_integer_with_truncation(&ctx, &day)?)?;
        let fields = CalendarFields::new().with_day(day);
        unwrap_temporal(&ctx, self.inner.to_plain_date(Some(fields))).map(PlainDate::wrap)
    }

    pub fn to_string<'js>(&self, options: Opt<Value<'js>>, ctx: Ctx<'js>) -> Result<String> {
        Ok(self.inner.to_ixdtf_string(display_calendar(&ctx, options)?))
    }

    pub fn to_locale_string<'js>(
        &self, _locales: Opt<Value<'js>>, _options: Opt<Value<'js>>,
    ) -> String {
        self.inner.to_ixdtf_string(DisplayCalendar::Auto)
    }

    #[qjs(rename = "toJSON")]
    pub fn to_json(&self) -> String { self.inner.to_ixdtf_string(DisplayCalendar::Auto) }

    pub fn value_of(&self, ctx: Ctx<'_>) -> Result<()> {
        Err(throw_value_of(&ctx, "Temporal.PlainYearMonth"))
    }

    #[qjs(prop, rename = PredefinedAtom::SymbolToStringTag, configurable)]
    pub const fn to_string_tag() -> &'static str { "Temporal.PlainYearMonth" }
}

struct RawYearMonthFields {
    year:       Option<i32>,
    month:      Option<i128>,
    month_code: Option<String>,
}

impl RawYearMonthFields {
    const fn is_empty(&self) -> bool {
        self.year.is_none() && self.month.is_none() && self.month_code.is_none()
    }

    fn into_fields(self, ctx: &Ctx<'_>, overflow: Overflow) -> Result<YearMonthCalendarFields> {
        let mut fields = YearMonthCalendarFields::new();
        if let Some(year) = self.year {
            fields = fields.with_year(year);
        }
        if let Some(month) = month_u8(ctx, self.month, overflow)? {
            fields = fields.with_month(month);
        }
        if let Some(code) = self.month_code {
            let month_code = MonthCode::try_from_utf8(code.as_bytes())
                .map_err(|error| crate::convert::throw_temporal(ctx, error))?;
            fields = fields.with_month_code(month_code);
        }
        Ok(fields)
    }
}

fn to_year_month<'js>(
    ctx: &Ctx<'js>, item: &Value<'js>, options: Opt<Value<'js>>,
) -> Result<temporal_rs::PlainYearMonth> {
    if let Some(year_month) = probe_class::<PlainYearMonth>(ctx, item) {
        let _overflow = overflow_option(ctx, options)?;
        return Ok(year_month.inner);
    }
    if item.is_string() {
        let string = item.get::<String>()?;
        let parsed = unwrap_temporal(
            ctx,
            temporal_rs::PlainYearMonth::from_utf8(string.as_bytes()),
        )?;
        let _overflow = overflow_option(ctx, options)?;
        return Ok(parsed);
    }
    let Some(object) = item.as_object() else {
        return Err(Exception::throw_type(
            ctx,
            "cannot convert value to Temporal.PlainYearMonth",
        ));
    };
    let calendar = object_calendar(ctx, item, object)?;
    let raw = year_month_fields(ctx, object)?;
    if raw.year.is_none() || (raw.month.is_none() && raw.month_code.is_none()) {
        return Err(Exception::throw_type(
            ctx,
            "year and month or monthCode are required",
        ));
    }
    let overflow = overflow_option(ctx, options)?;
    let fields = raw.into_fields(ctx, overflow.unwrap_or_default())?;
    unwrap_temporal(
        ctx,
        temporal_rs::PlainYearMonth::from_partial(
            PartialYearMonth {
                calendar_fields: fields,
                calendar,
            },
            overflow,
        ),
    )
}

fn object_calendar<'js>(
    ctx: &Ctx<'js>, item: &Value<'js>, object: &Object<'js>,
) -> Result<Calendar> {
    if let Some(calendar) = calendar_slot(ctx, item) {
        return Ok(calendar);
    }
    calendar_from_value(ctx, &object.get("calendar")?, true)
}

fn calendar_from_value<'js>(
    ctx: &Ctx<'js>, value: &Value<'js>, parse_iso_string: bool,
) -> Result<Calendar> {
    if let Some(calendar) = calendar_slot(ctx, value) {
        return Ok(calendar);
    }
    if value.is_undefined() {
        return Ok(Calendar::ISO);
    }
    if !value.is_string() {
        return Err(Exception::throw_type(
            ctx,
            "calendar must be a calendar identifier string",
        ));
    }
    let identifier = value.get::<String>()?;
    if parse_iso_string {
        unwrap_temporal(ctx, Calendar::from_str(&identifier))
    } else {
        unwrap_temporal(ctx, Calendar::try_from_utf8(identifier.as_bytes()))
    }
}

fn canonicalize_calendar<'js>(ctx: &Ctx<'js>, calendar: Opt<Value<'js>>) -> Result<Calendar> {
    match calendar.0 {
        None => Ok(Calendar::ISO),
        Some(value) if value.is_undefined() => Ok(Calendar::ISO),
        Some(value) => calendar_from_value(ctx, &value, false),
    }
}

fn year_month_fields<'js>(ctx: &Ctx<'js>, object: &Object<'js>) -> Result<RawYearMonthFields> {
    let month = optional_truncated_i128(ctx, object, "month")?;
    if matches!(month, Some(value) if value < 1) {
        return Err(Exception::throw_range(ctx, "integer is out of range"));
    }
    let month_code = optional_month_code(ctx, &object.get("monthCode")?)?;
    let year = optional_truncated_i32(ctx, object, "year")?;
    Ok(RawYearMonthFields {
        month,
        month_code,
        year,
    })
}

fn is_temporal_object<'js>(ctx: &Ctx<'js>, value: &Value<'js>) -> bool {
    calendar_slot(ctx, value).is_some()
        || probe_class::<PlainTime>(ctx, value).is_some()
        || probe_class::<Instant>(ctx, value).is_some()
        || probe_class::<Duration>(ctx, value).is_some()
}

fn overflow_option<'js>(ctx: &Ctx<'js>, options: Opt<Value<'js>>) -> Result<Option<Overflow>> {
    options_object(ctx, options)?.map_or(Ok(None), |object| {
        get_defined(&object, "overflow")?.map_or(Ok(None), |value| {
            parse_enum(ctx, &value, "invalid overflow option").map(Some)
        })
    })
}

fn display_calendar<'js>(ctx: &Ctx<'js>, options: Opt<Value<'js>>) -> Result<DisplayCalendar> {
    options_object(ctx, options)?.map_or(Ok(DisplayCalendar::Auto), |object| {
        get_defined(&object, "calendarName")?.map_or(Ok(DisplayCalendar::Auto), |value| {
            parse_enum(ctx, &value, "invalid calendarName option")
        })
    })
}

fn difference_settings<'js>(
    ctx: &Ctx<'js>, options: Opt<Value<'js>>,
) -> Result<DifferenceSettings> {
    let Some(object) = options_object(ctx, options)? else {
        return Ok(DifferenceSettings::default());
    };
    let mut settings = DifferenceSettings::default();
    settings.largest_unit = optional_unit(ctx, &object.get("largestUnit")?)?;
    settings.increment = optional_rounding_increment(ctx, &object.get("roundingIncrement")?)?;
    settings.rounding_mode = optional_rounding_mode(ctx, &object.get("roundingMode")?)?;
    settings.smallest_unit = optional_unit(ctx, &object.get("smallestUnit")?)?;
    Ok(settings)
}

fn optional_unit<'js>(ctx: &Ctx<'js>, value: &Value<'js>) -> Result<Option<Unit>> {
    if value.is_undefined() {
        Ok(None)
    } else {
        parse_enum(ctx, value, "invalid Temporal unit").map(Some)
    }
}

fn optional_rounding_mode<'js>(ctx: &Ctx<'js>, value: &Value<'js>) -> Result<Option<RoundingMode>> {
    if value.is_undefined() {
        Ok(None)
    } else {
        parse_enum(ctx, value, "invalid roundingMode").map(Some)
    }
}

fn parse_enum<'js, T: FromStr>(ctx: &Ctx<'js>, value: &Value<'js>, message: &str) -> Result<T> {
    let name = option_string(ctx, value)?;
    name.parse()
        .map_err(|_error| Exception::throw_range(ctx, message))
}

fn option_string<'js>(ctx: &Ctx<'js>, value: &Value<'js>) -> Result<String> {
    if let Some(string) = value.as_string() {
        return string.to_string();
    }
    if value.is_symbol() {
        return Err(Exception::throw_type(
            ctx,
            "cannot convert Symbol to a string",
        ));
    }
    if let Some(object) = value.as_object() {
        let to_string = object.get::<_, Value>("toString")?;
        if let Some(function) = to_string.as_function() {
            let primitive: Value = function.call((rquickjs::function::This(value.clone()),))?;
            if let Some(string) = primitive.as_string() {
                return string.to_string();
            }
            if primitive.is_symbol() {
                return Err(Exception::throw_type(
                    ctx,
                    "cannot convert Symbol to a string",
                ));
            }
            return option_string(ctx, &primitive);
        }
    }
    if value.is_null() {
        return Ok("null".to_string());
    }
    if value.is_bool() {
        return Ok(if value.as_bool() == Some(true) {
            "true".to_string()
        } else {
            "false".to_string()
        });
    }
    if value.is_number() {
        return Ok(to_number(ctx, value)?.to_string());
    }
    if value.is_big_int() {
        return crate::convert::js_to_string(ctx, value);
    }
    crate::convert::js_to_string(ctx, value)
}

fn optional_rounding_increment<'js>(
    ctx: &Ctx<'js>, value: &Value<'js>,
) -> Result<Option<RoundingIncrement>> {
    if value.is_undefined() {
        Ok(None)
    } else {
        let number = to_number(ctx, value)?;
        unwrap_temporal(ctx, RoundingIncrement::try_from(number)).map(Some)
    }
}

fn optional_month_code<'js>(ctx: &Ctx<'js>, value: &Value<'js>) -> Result<Option<String>> {
    if value.is_undefined() {
        return Ok(None);
    }
    let code = require_string(ctx, value)?;
    reject_illformed_month_code(ctx, &code)?;
    Ok(Some(code))
}

fn require_string<'js>(ctx: &Ctx<'js>, value: &Value<'js>) -> Result<String> {
    if let Some(string) = value.as_string() {
        return string.to_string();
    }
    if let Some(object) = value.as_object() {
        let to_string = object.get::<_, Value>("toString")?;
        if let Some(function) = to_string.as_function() {
            let primitive: Value = function.call((rquickjs::function::This(value.clone()),))?;
            if let Some(string) = primitive.as_string() {
                return string.to_string();
            }
        }
    }
    Err(Exception::throw_type(ctx, "monthCode must be a string"))
}

fn month_u8(ctx: &Ctx<'_>, month: Option<i128>, overflow: Overflow) -> Result<Option<u8>> {
    let Some(month) = month else {
        return Ok(None);
    };
    if month < 0 {
        return Err(Exception::throw_range(ctx, "integer is out of range"));
    }
    if overflow == Overflow::Constrain && month > 12 {
        return Ok(Some(12));
    }
    u8::try_from(month)
        .map(Some)
        .map_err(|_error| Exception::throw_range(ctx, "integer is out of range"))
}

fn constrain_u8(ctx: &Ctx<'_>, integer: i128) -> Result<u8> {
    if integer < 0 {
        return Err(Exception::throw_range(ctx, "integer is out of range"));
    }
    u8::try_from(integer).or(Ok(u8::MAX))
}
