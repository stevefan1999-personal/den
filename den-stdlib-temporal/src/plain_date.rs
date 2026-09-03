use std::str::FromStr;

use rquickjs::{
    Coerced, Ctx, Exception, FromJs as _, JsLifetime, Object, Result, Value, atom::PredefinedAtom,
    class::Trace, prelude::Opt,
};
use temporal_rs::{
    Calendar, MonthCode,
    fields::CalendarFields,
    options::{
        DifferenceSettings, DisplayCalendar, Overflow, RoundingIncrement, RoundingMode, Unit,
    },
    partial::PartialDate,
};

use crate::{
    convert::{
        calendar_slot, ctor_required_i32, ctor_required_u8, get_defined, optional_month_code,
        optional_truncated_i32, optional_truncated_i128, options_object, probe_class,
        throw_value_of, to_duration, to_number, to_time_zone, unwrap_temporal,
    },
    duration::Duration,
    plain_date_time::PlainDateTime,
    plain_month_day::PlainMonthDay,
    plain_time::{PlainTime, to_temporal_time},
    plain_year_month::PlainYearMonth,
    zoned_date_time::ZonedDateTime,
};

#[derive(Trace, JsLifetime, Clone)]
#[rquickjs::class(rename = "PlainDate", frozen)]
pub struct PlainDate {
    #[qjs(skip_trace)]
    pub(crate) inner: temporal_rs::PlainDate,
}

impl PlainDate {
    pub(crate) const fn wrap(inner: temporal_rs::PlainDate) -> Self { Self { inner } }
}

fn option_enum<'js, T: FromStr>(ctx: &Ctx<'js>, value: &Value<'js>, what: &str) -> Result<T> {
    let name = Coerced::<String>::from_js(ctx, value.clone())?.0;
    T::from_str(&name).map_err(|_error| Exception::throw_range(ctx, &format!("invalid {what}")))
}

fn overflow_option<'js>(ctx: &Ctx<'js>, options: Opt<Value<'js>>) -> Result<Option<Overflow>> {
    let Some(object) = options_object(ctx, options)? else {
        return Ok(None);
    };
    get_defined(&object, "overflow")?.map_or(Ok(None), |value| {
        option_enum(ctx, &value, "overflow option").map(Some)
    })
}

fn difference_settings<'js>(
    ctx: &Ctx<'js>, options: Opt<Value<'js>>,
) -> Result<DifferenceSettings> {
    let Some(object) = options_object(ctx, options)? else {
        return Ok(DifferenceSettings::default());
    };
    let largest_unit = match get_defined(&object, "largestUnit")? {
        None => None,
        Some(value) => Some(option_enum::<Unit>(ctx, &value, "Temporal unit")?),
    };
    let increment = match get_defined(&object, "roundingIncrement")? {
        None => None,
        Some(value) => {
            let number = to_number(ctx, &value)?;
            Some(unwrap_temporal(ctx, RoundingIncrement::try_from(number))?)
        }
    };
    let rounding_mode = match get_defined(&object, "roundingMode")? {
        None => None,
        Some(value) => Some(option_enum::<RoundingMode>(ctx, &value, "roundingMode")?),
    };
    let smallest_unit = match get_defined(&object, "smallestUnit")? {
        None => None,
        Some(value) => Some(option_enum::<Unit>(ctx, &value, "Temporal unit")?),
    };
    let mut settings = DifferenceSettings::default();
    settings.largest_unit = largest_unit;
    settings.smallest_unit = smallest_unit;
    settings.rounding_mode = rounding_mode;
    settings.increment = increment;
    Ok(settings)
}

fn calendar_from_value<'js>(ctx: &Ctx<'js>, value: &Value<'js>) -> Result<Calendar> {
    if let Some(calendar) = calendar_slot(ctx, value) {
        return Ok(calendar);
    }
    if value.is_string() {
        let identifier: String = value.get()?;
        return unwrap_temporal(ctx, Calendar::from_str(&identifier));
    }
    Err(Exception::throw_type(
        ctx,
        "calendar must be a calendar identifier string",
    ))
}

fn optional_positive_date_unit<'js>(
    ctx: &Ctx<'js>, object: &Object<'js>, key: &str,
) -> Result<Option<i128>> {
    match optional_truncated_i128(ctx, object, key)? {
        None => Ok(None),
        Some(value) if value < 1 => {
            Err(Exception::throw_range(
                ctx,
                "month and day must be positive",
            ))
        }
        Some(value) => Ok(Some(value)),
    }
}

fn u8_date_unit(ctx: &Ctx<'_>, value: i128, overflow: Overflow) -> Result<u8> {
    if value < 1 {
        return Err(Exception::throw_range(
            ctx,
            "month and day must be positive",
        ));
    }
    u8::try_from(value).map_or_else(
        |_error| {
            match overflow {
                Overflow::Constrain => Ok(u8::MAX),
                Overflow::Reject => Err(Exception::throw_range(ctx, "date unit is out of range")),
            }
        },
        Ok,
    )
}

struct DateBag {
    calendar:   Calendar,
    year:       Option<i32>,
    month:      Option<i128>,
    month_code: Option<String>,
    day:        Option<i128>,
}

impl DateBag {
    const fn is_empty(&self) -> bool {
        self.year.is_none()
            && self.month.is_none()
            && self.month_code.is_none()
            && self.day.is_none()
    }

    fn calendar_fields(&self, ctx: &Ctx<'_>, overflow: Overflow) -> Result<CalendarFields> {
        let mut fields = CalendarFields::new();
        if let Some(year) = self.year {
            fields = fields.with_year(year);
        }
        if let Some(month) = self.month {
            fields = fields.with_month(u8_date_unit(ctx, month, overflow)?);
        }
        if let Some(day) = self.day {
            fields = fields.with_day(u8_date_unit(ctx, day, overflow)?);
        }
        if let Some(month_code) = &self.month_code {
            let month_code = unwrap_temporal(ctx, MonthCode::try_from_utf8(month_code.as_bytes()))?;
            fields = fields.with_month_code(month_code);
        }
        Ok(fields)
    }
}

fn read_date_bag<'js>(
    ctx: &Ctx<'js>, object: &Object<'js>, reject_calendar_or_time_zone: bool,
) -> Result<DateBag> {
    let calendar_value = get_defined(object, "calendar")?;
    if reject_calendar_or_time_zone {
        if calendar_value.is_some() {
            return Err(Exception::throw_type(
                ctx,
                "calendar is not allowed on a partial date",
            ));
        }
        if get_defined(object, "timeZone")?.is_some() {
            return Err(Exception::throw_type(
                ctx,
                "timeZone is not allowed on a partial date",
            ));
        }
    }
    let calendar = match calendar_value {
        None => Calendar::ISO,
        Some(value) => calendar_from_value(ctx, &value)?,
    };
    let day = optional_positive_date_unit(ctx, object, "day")?;
    let month = optional_positive_date_unit(ctx, object, "month")?;
    let month_code = optional_month_code(ctx, object)?;
    let year = optional_truncated_i32(ctx, object, "year")?;
    Ok(DateBag {
        calendar,
        year,
        month,
        month_code,
        day,
    })
}

fn is_temporal_date_like<'js>(ctx: &Ctx<'js>, value: &Value<'js>) -> bool {
    calendar_slot(ctx, value).is_some() || probe_class::<PlainTime>(ctx, value).is_some()
}

fn date_from_partial(
    ctx: &Ctx<'_>, bag: DateBag, overflow: Overflow,
) -> Result<temporal_rs::PlainDate> {
    let partial = PartialDate {
        calendar_fields: bag.calendar_fields(ctx, overflow)?,
        calendar:        bag.calendar,
    };
    unwrap_temporal(
        ctx,
        temporal_rs::PlainDate::from_partial(partial, Some(overflow)),
    )
}

fn to_temporal_date<'js>(ctx: &Ctx<'js>, value: &Value<'js>) -> Result<temporal_rs::PlainDate> {
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
        let string: String = value.get()?;
        return unwrap_temporal(ctx, temporal_rs::PlainDate::from_utf8(string.as_bytes()));
    }
    let object = value
        .as_object()
        .ok_or_else(|| Exception::throw_type(ctx, "cannot convert value to Temporal.PlainDate"))?;
    let bag = read_date_bag(ctx, object, false)?;
    date_from_partial(ctx, bag, Overflow::Constrain)
}

#[rquickjs::methods(rename_all = "camelCase")]
impl PlainDate {
    #[qjs(constructor)]
    pub fn new<'js>(
        iso_year: Opt<Value<'js>>, iso_month: Opt<Value<'js>>, iso_day: Opt<Value<'js>>,
        calendar: Opt<Value<'js>>, ctx: Ctx<'js>,
    ) -> Result<Self> {
        let year = ctor_required_i32(&ctx, iso_year)?;
        let month = ctor_required_u8(&ctx, iso_month)?;
        let day = ctor_required_u8(&ctx, iso_day)?;
        let calendar = match calendar.0.filter(|value| !value.is_undefined()) {
            None => Calendar::ISO,
            Some(value) if value.is_string() => {
                let identifier: String = value.get()?;
                unwrap_temporal(&ctx, Calendar::try_from_utf8(identifier.as_bytes()))?
            }
            Some(value) => calendar_from_value(&ctx, &value)?,
        };
        unwrap_temporal(
            &ctx,
            temporal_rs::PlainDate::try_new(year, month, day, calendar),
        )
        .map(Self::wrap)
    }

    #[qjs(static)]
    pub fn from<'js>(item: Value<'js>, options: Opt<Value<'js>>, ctx: Ctx<'js>) -> Result<Self> {
        if let Some(date) = probe_class::<PlainDate>(&ctx, &item) {
            let _overflow = overflow_option(&ctx, options)?;
            return Ok(Self::wrap(date.inner));
        }
        if let Some(date_time) = probe_class::<PlainDateTime>(&ctx, &item) {
            let _overflow = overflow_option(&ctx, options)?;
            return Ok(Self::wrap(date_time.inner.to_plain_date()));
        }
        if let Some(zoned) = probe_class::<ZonedDateTime>(&ctx, &item) {
            let _overflow = overflow_option(&ctx, options)?;
            return Ok(Self::wrap(zoned.inner.to_plain_date()));
        }
        if item.is_string() {
            let string: String = item.get()?;
            let date = unwrap_temporal(&ctx, temporal_rs::PlainDate::from_utf8(string.as_bytes()))?;
            let _overflow = overflow_option(&ctx, options)?;
            return Ok(Self::wrap(date));
        }
        let Some(object) = item.as_object() else {
            return Err(Exception::throw_type(
                &ctx,
                "cannot convert value to Temporal.PlainDate",
            ));
        };
        let bag = read_date_bag(&ctx, object, false)?;
        let overflow = overflow_option(&ctx, options)?.unwrap_or(Overflow::Constrain);
        date_from_partial(&ctx, bag, overflow).map(Self::wrap)
    }

    #[qjs(static)]
    pub fn compare<'js>(one: Value<'js>, two: Value<'js>, ctx: Ctx<'js>) -> Result<i32> {
        let left = to_temporal_date(&ctx, &one)?;
        let right = to_temporal_date(&ctx, &two)?;
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
        let other = to_temporal_date(&ctx, &other)?;
        let settings = difference_settings(&ctx, options)?;
        unwrap_temporal(&ctx, self.inner.until(&other, settings)).map(Duration::wrap)
    }

    pub fn since<'js>(
        &self, other: Value<'js>, options: Opt<Value<'js>>, ctx: Ctx<'js>,
    ) -> Result<Duration> {
        let other = to_temporal_date(&ctx, &other)?;
        let settings = difference_settings(&ctx, options)?;
        unwrap_temporal(&ctx, self.inner.since(&other, settings)).map(Duration::wrap)
    }

    pub fn with<'js>(
        &self, temporal_date_like: Value<'js>, options: Opt<Value<'js>>, ctx: Ctx<'js>,
    ) -> Result<Self> {
        if !temporal_date_like.is_object() {
            return Err(Exception::throw_type(
                &ctx,
                "with() requires a property bag",
            ));
        }
        if is_temporal_date_like(&ctx, &temporal_date_like) {
            return Err(Exception::throw_type(
                &ctx,
                "with() does not accept a Temporal object",
            ));
        }
        let object = temporal_date_like
            .as_object()
            .ok_or_else(|| Exception::throw_type(&ctx, "with() requires a property bag"))?;
        let bag = read_date_bag(&ctx, object, true)?;
        if bag.is_empty() {
            return Err(Exception::throw_type(
                &ctx,
                "with() requires at least one date field",
            ));
        }
        let overflow = overflow_option(&ctx, options)?.unwrap_or(Overflow::Constrain);
        let fields = bag.calendar_fields(&ctx, overflow)?;
        unwrap_temporal(&ctx, self.inner.with(fields, Some(overflow))).map(Self::wrap)
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

    pub fn equals<'js>(&self, other: Value<'js>, ctx: Ctx<'js>) -> Result<bool> {
        Ok(self.inner == to_temporal_date(&ctx, &other)?)
    }

    pub fn to_plain_date_time<'js>(
        &self, time: Opt<Value<'js>>, ctx: Ctx<'js>,
    ) -> Result<PlainDateTime> {
        let time = time
            .0
            .filter(|value| !value.is_undefined())
            .map(|value| to_temporal_time(&ctx, &value))
            .transpose()?;
        unwrap_temporal(&ctx, self.inner.to_plain_date_time(time)).map(PlainDateTime::wrap)
    }

    pub fn to_plain_year_month(&self, ctx: Ctx<'_>) -> Result<PlainYearMonth> {
        unwrap_temporal(&ctx, self.inner.to_plain_year_month()).map(PlainYearMonth::wrap)
    }

    pub fn to_plain_month_day(&self, ctx: Ctx<'_>) -> Result<PlainMonthDay> {
        unwrap_temporal(&ctx, self.inner.to_plain_month_day()).map(PlainMonthDay::wrap)
    }

    pub fn to_zoned_date_time<'js>(
        &self, item: Value<'js>, ctx: Ctx<'js>,
    ) -> Result<ZonedDateTime> {
        let (time_zone, plain_time) = if item.is_string() {
            (to_time_zone(&ctx, &item)?, None)
        } else if let Some(object) = item.as_object() {
            let zone = match get_defined(object, "timeZone")? {
                Some(value) => to_time_zone(&ctx, &value)?,
                None => {
                    return Err(Exception::throw_type(&ctx, "timeZone is required"));
                }
            };
            let time = match get_defined(object, "plainTime")? {
                Some(value) => Some(to_temporal_time(&ctx, &value)?),
                None => None,
            };
            (zone, time)
        } else {
            return Err(Exception::throw_type(&ctx, "timeZone is required"));
        };
        unwrap_temporal(&ctx, self.inner.to_zoned_date_time(time_zone, plain_time))
            .map(ZonedDateTime::wrap)
    }

    pub fn to_string<'js>(&self, options: Opt<Value<'js>>, ctx: Ctx<'js>) -> Result<String> {
        let display = match options_object(&ctx, options)? {
            None => DisplayCalendar::Auto,
            Some(object) => {
                match get_defined(&object, "calendarName")? {
                    None => DisplayCalendar::Auto,
                    Some(value) => option_enum(&ctx, &value, "calendarName option")?,
                }
            }
        };
        Ok(self.inner.to_ixdtf_string(display))
    }

    #[qjs(rename = "toJSON")]
    pub fn to_json(&self) -> String { self.inner.to_ixdtf_string(DisplayCalendar::Auto) }

    pub fn value_of(&self, ctx: Ctx<'_>) -> Result<()> {
        Err(throw_value_of(&ctx, "Temporal.PlainDate"))
    }

    #[qjs(prop, rename = PredefinedAtom::SymbolToStringTag, configurable)]
    pub const fn to_string_tag() -> &'static str { "Temporal.PlainDate" }
}
