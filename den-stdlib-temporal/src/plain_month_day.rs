use std::str::FromStr as _;

use rquickjs::{
    Ctx, Exception, JsLifetime, Object, Result, Value, atom::PredefinedAtom, class::Trace,
    prelude::Opt,
};
use temporal_rs::{
    Calendar, MonthCode,
    fields::CalendarFields,
    options::{DisplayCalendar, Overflow},
    partial::PartialDate,
};

use crate::{
    convert::{
        calendar_name_option, calendar_slot, ctor_required_u8, get_defined, optional_month_code,
        overflow_option, probe_class, require_object, throw_temporal, throw_value_of,
        to_integer_with_truncation, to_plain_month_day, truncated_i32, unwrap_temporal,
    },
    plain_date::PlainDate,
    plain_time::PlainTime,
};

#[derive(Trace, JsLifetime, Clone)]
#[rquickjs::class(rename = "PlainMonthDay", frozen)]
pub struct PlainMonthDay {
    #[qjs(skip_trace)]
    pub(crate) inner: temporal_rs::PlainMonthDay,
}

impl PlainMonthDay {
    pub(crate) const fn wrap(inner: temporal_rs::PlainMonthDay) -> Self { Self { inner } }
}

#[rquickjs::methods(rename_all = "camelCase")]
impl PlainMonthDay {
    #[qjs(constructor)]
    pub fn new<'js>(
        iso_month: Opt<Value<'js>>, iso_day: Opt<Value<'js>>, calendar: Opt<Value<'js>>,
        reference_iso_year: Opt<Value<'js>>, ctx: Ctx<'js>,
    ) -> Result<Self> {
        let month = ctor_required_u8(&ctx, iso_month)?;
        let day = ctor_required_u8(&ctx, iso_day)?;
        let calendar = ctor_calendar(&ctx, calendar)?;
        let reference_year = match reference_iso_year.0.filter(|value| !value.is_undefined()) {
            None => None,
            Some(value) => Some(truncated_i32(&ctx, &value)?),
        };
        unwrap_temporal(
            &ctx,
            temporal_rs::PlainMonthDay::new_with_overflow(
                month,
                day,
                calendar,
                Overflow::Reject,
                reference_year,
            ),
        )
        .map(Self::wrap)
    }

    #[qjs(static)]
    pub fn from<'js>(item: Value<'js>, options: Opt<Value<'js>>, ctx: Ctx<'js>) -> Result<Self> {
        to_temporal_month_day(&ctx, &item, options).map(Self::wrap)
    }

    #[qjs(get, configurable)]
    pub fn calendar_id(&self) -> &'static str { self.inner.calendar_id() }

    #[qjs(get, configurable)]
    pub fn month_code(&self) -> String { self.inner.month_code().as_str().to_string() }

    #[qjs(get, configurable)]
    pub fn day(&self) -> u8 { self.inner.day() }

    pub fn with<'js>(
        &self, item: Value<'js>, options: Opt<Value<'js>>, ctx: Ctx<'js>,
    ) -> Result<Self> {
        if !is_partial_temporal_object(&ctx, &item)? {
            return Err(Exception::throw_type(
                &ctx,
                "with() requires a PlainMonthDay-like object",
            ));
        }
        let Some(object) = item.as_object().cloned() else {
            return Err(Exception::throw_type(
                &ctx,
                "with() requires a PlainMonthDay-like object",
            ));
        };
        let bag = month_day_bag(&ctx, &object)?;
        if bag.is_empty() {
            return Err(Exception::throw_type(
                &ctx,
                "with() requires a calendar field",
            ));
        }
        let overflow = overflow_option(&ctx, options)?;
        unwrap_temporal(&ctx, self.inner.with(bag.into_fields(&ctx)?, overflow)).map(Self::wrap)
    }

    pub fn equals<'js>(&self, other: Value<'js>, ctx: Ctx<'js>) -> Result<bool> {
        Ok(self.inner == to_temporal_month_day(&ctx, &other, Opt(None))?)
    }

    pub fn to_plain_date<'js>(
        &self, item: Value<'js>, _options: Opt<Value<'js>>, ctx: Ctx<'js>,
    ) -> Result<PlainDate> {
        let object = require_object(&ctx, &item, "toPlainDate() requires an object")?;
        let Some(year) = get_defined(&object, "year")? else {
            return Err(Exception::throw_type(&ctx, "year is required"));
        };
        let fields = CalendarFields::new().with_year(truncated_i32(&ctx, &year)?);
        unwrap_temporal(&ctx, self.inner.to_plain_date(Some(fields))).map(PlainDate::wrap)
    }

    pub fn to_string<'js>(&self, options: Opt<Value<'js>>, ctx: Ctx<'js>) -> Result<String> {
        Ok(self
            .inner
            .to_ixdtf_string(calendar_name_option(&ctx, options)?))
    }

    #[qjs(rename = "toJSON")]
    pub fn to_json(&self) -> String { self.inner.to_ixdtf_string(DisplayCalendar::Auto) }

    pub fn value_of(&self, ctx: Ctx<'_>) -> Result<()> {
        Err(throw_value_of(&ctx, "Temporal.PlainMonthDay"))
    }

    #[qjs(prop, rename = PredefinedAtom::SymbolToStringTag, configurable)]
    pub const fn to_string_tag() -> &'static str { "Temporal.PlainMonthDay" }
}

struct MonthDayBag {
    day:        Option<u8>,
    month:      Option<u8>,
    month_code: Option<String>,
    year:       Option<i32>,
}

impl MonthDayBag {
    const fn is_empty(&self) -> bool {
        self.day.is_none()
            && self.month.is_none()
            && self.month_code.is_none()
            && self.year.is_none()
    }

    fn into_fields(self, ctx: &Ctx<'_>) -> Result<CalendarFields> {
        let mut fields = CalendarFields::new();
        if let Some(day) = self.day {
            fields = fields.with_day(day);
        }
        if let Some(month) = self.month {
            fields = fields.with_month(month);
        }
        if let Some(month_code) = self.month_code {
            let month_code = MonthCode::try_from_utf8(month_code.as_bytes())
                .map_err(|error| throw_temporal(ctx, error))?;
            fields = fields.with_month_code(month_code);
        }
        if let Some(year) = self.year {
            fields = fields.with_year(year);
        }
        Ok(fields)
    }
}

fn to_temporal_month_day<'js>(
    ctx: &Ctx<'js>, item: &Value<'js>, options: Opt<Value<'js>>,
) -> Result<temporal_rs::PlainMonthDay> {
    if probe_class::<PlainMonthDay>(ctx, item).is_some() || item.is_string() {
        let inner = to_plain_month_day(ctx, item)?;
        overflow_option(ctx, options)?;
        return Ok(inner);
    }
    if let Some(object) = item.as_object().cloned() {
        let calendar = calendar_from_item(ctx, &object)?;
        let bag = month_day_bag(ctx, &object)?;
        let overflow = overflow_option(ctx, options)?;
        if bag.month.is_none() && bag.month_code.is_none() {
            return Err(Exception::throw_type(ctx, "month or monthCode is required"));
        }
        if bag.day.is_none() {
            return Err(Exception::throw_type(ctx, "day is required"));
        }
        return unwrap_temporal(
            ctx,
            temporal_rs::PlainMonthDay::from_partial(
                PartialDate {
                    calendar_fields: bag.into_fields(ctx)?,
                    calendar,
                },
                overflow,
            ),
        );
    }
    Err(Exception::throw_type(
        ctx,
        "cannot convert value to Temporal.PlainMonthDay",
    ))
}

fn month_day_bag<'js>(ctx: &Ctx<'js>, object: &Object<'js>) -> Result<MonthDayBag> {
    let day = match get_defined(object, "day")? {
        None => None,
        Some(value) => Some(field_to_u8(ctx, &value)?),
    };
    let month = match get_defined(object, "month")? {
        None => None,
        Some(value) => Some(field_to_u8(ctx, &value)?),
    };
    let month_code = optional_month_code(ctx, object)?;
    let year = match get_defined(object, "year")? {
        None => None,
        Some(value) => Some(truncated_i32(ctx, &value)?),
    };
    Ok(MonthDayBag {
        day,
        month,
        month_code,
        year,
    })
}

fn calendar_from_item<'js>(ctx: &Ctx<'js>, object: &Object<'js>) -> Result<Calendar> {
    get_defined(object, "calendar")?
        .map_or(Ok(Calendar::ISO), |value| calendar_from_value(ctx, &value))
}

fn calendar_from_value<'js>(ctx: &Ctx<'js>, value: &Value<'js>) -> Result<Calendar> {
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
    let identifier: String = value.get()?;
    Calendar::from_str(&identifier).map_err(|error| throw_temporal(ctx, error))
}

fn ctor_calendar<'js>(ctx: &Ctx<'js>, calendar: Opt<Value<'js>>) -> Result<Calendar> {
    match calendar.0.filter(|value| !value.is_undefined()) {
        None => Ok(Calendar::ISO),
        Some(value) => {
            if !value.is_string() {
                return Err(Exception::throw_type(
                    ctx,
                    "calendar must be a calendar identifier string",
                ));
            }
            let identifier: String = value.get()?;
            Calendar::try_from_utf8(identifier.as_bytes())
                .map_err(|error| throw_temporal(ctx, error))
        }
    }
}

fn field_to_u8<'js>(ctx: &Ctx<'js>, value: &Value<'js>) -> Result<u8> {
    let integer = to_integer_with_truncation(ctx, value)?;
    if integer <= 0 {
        return Err(Exception::throw_range(ctx, "integer is out of range"));
    }
    if integer > i128::from(u8::MAX) {
        return Ok(u8::MAX);
    }
    Ok(integer as u8)
}

fn is_partial_temporal_object<'js>(ctx: &Ctx<'js>, value: &Value<'js>) -> Result<bool> {
    let Some(object) = value.as_object() else {
        return Ok(false);
    };
    if calendar_slot(ctx, value).is_some() || probe_class::<PlainTime>(ctx, value).is_some() {
        return Ok(false);
    }
    if get_defined(object, "calendar")?.is_some() || get_defined(object, "timeZone")?.is_some() {
        return Ok(false);
    }
    Ok(true)
}
