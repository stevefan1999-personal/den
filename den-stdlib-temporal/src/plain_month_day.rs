use std::str::FromStr;

use den_util::ObjectExt as _;
use rquickjs::{
    Ctx, Exception, Filter, Function, JsLifetime, Object, Result, Value,
    atom::PredefinedAtom,
    class::{
        Trace,
        impl_::{ConstructorCreate, ConstructorCreator},
    },
    function::{Constructor, This},
    object::{Accessor, Property},
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
        calendar_slot, ctor_required_u8, get_defined, options_object, probe_class,
        reject_illformed_month_code, require_object, throw_temporal, throw_value_of,
        to_integer_with_truncation, to_js_string, to_plain_month_day, truncated_i32,
        unwrap_temporal,
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
    pub(crate) fn wrap(inner: temporal_rs::PlainMonthDay) -> Self { Self { inner } }

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

    pub fn from<'js>(item: Value<'js>, options: Opt<Value<'js>>, ctx: Ctx<'js>) -> Result<Self> {
        to_temporal_month_day(&ctx, &item, options).map(Self::wrap)
    }
}

impl<'js> ConstructorCreator<'js, PlainMonthDay> for ConstructorCreate<PlainMonthDay> {
    fn create_constructor(&self, ctx: &Ctx<'js>) -> Result<Option<Constructor<'js>>> {
        let constr =
            Constructor::new_class::<PlainMonthDay, _, _>(ctx.clone(), PlainMonthDay::new)?;
        let func: &Function = constr.as_ref();
        func.set_length(2)?;
        let from = Function::new(ctx.clone(), PlainMonthDay::from)?.with_name("from")?;
        let object: &Object = func.as_ref();
        object.prop("from", Property::from(from).writable().configurable())?;
        // lib.rs replaces the constructor with a NewTarget wrapper whose
        // [[Prototype]] is this original. Copy own statics onto that wrapper
        // and restore Function.prototype when it writes `.constructor`.
        install_new_target_trap(ctx, &constr)?;
        Ok(Some(constr))
    }
}

fn install_new_target_trap<'js>(ctx: &Ctx<'js>, original: &Constructor<'js>) -> Result<()> {
    let proto: Object = original.get(PredefinedAtom::Prototype)?;
    proto.prop(
        PredefinedAtom::Constructor,
        Accessor::new(
            move |ctx: Ctx<'js>| rust_plain_month_day(&ctx),
            move |ctx: Ctx<'js>, wrapped: Value<'js>| -> Result<()> {
                let original = rust_plain_month_day(&ctx)?;
                if let Some(object) = as_object_like(&wrapped) {
                    copy_statics(&original, &object)?;
                }
                let proto: Object = original.get(PredefinedAtom::Prototype)?;
                proto.prop(
                    PredefinedAtom::Constructor,
                    Property::from(wrapped).writable().configurable(),
                )?;
                Ok(())
            },
        )
        .configurable(),
    )?;

    let globals = ctx.globals();
    let existing = crate::shape::get_own_descriptor(&globals, "Temporal")?;
    let existing_get = existing
        .as_ref()
        .and_then(|desc| desc.get::<_, Function>("get").ok());
    let existing_value: Option<Value> = existing.as_ref().and_then(|desc| desc.get("value").ok());
    globals.prop(
        "Temporal",
        Accessor::new(
            move |ctx: Ctx<'js>| -> Result<Value<'js>> {
                if let Some(get) = existing_get.clone() {
                    return get.call((This(ctx.globals()),));
                }
                Ok(existing_value
                    .clone()
                    .unwrap_or_else(|| Value::new_undefined(ctx.clone())))
            },
            move |ctx: Ctx<'js>, ns: Value<'js>| -> Result<()> {
                let ctor = match as_object_like(&ns) {
                    Some(object) => object.get("PlainMonthDay")?,
                    None => Value::new_undefined(ctx.clone()),
                };
                let original = rust_plain_month_day(&ctx)?;
                finish_plain_month_day(&ctx, &original, ctor)?;
                ctx.globals()
                    .prop("Temporal", Property::from(ns).writable().configurable())?;
                Ok(())
            },
        )
        .configurable(),
    )?;
    Ok(())
}

fn rust_plain_month_day<'js>(ctx: &Ctx<'js>) -> Result<Constructor<'js>> {
    crate::shape::original_constructor(ctx, "PlainMonthDay")
}

fn finish_plain_month_day<'js>(
    ctx: &Ctx<'js>, original: &Constructor<'js>, ctor: Value<'js>,
) -> Result<()> {
    let Some(ctor_obj) = as_object_like(&ctor) else {
        return Ok(());
    };
    copy_statics(original, &ctor_obj)?;
    ctor_obj.set_prototype(Some(&Function::prototype(ctx.clone())))?;
    let proto: Object = ctor_obj.get(PredefinedAtom::Prototype)?;
    let proto_names: Vec<String> = proto
        .own_keys::<String>(Filter::new().string())
        .collect::<Result<_>>()?;
    for name in proto_names {
        if name == "constructor" {
            continue;
        }
        let Some(desc) = crate::shape::get_own_descriptor(&proto, &name)? else {
            continue;
        };
        let value: Value = desc.get("value")?;
        if let Some(func) = value.as_function() {
            func.set_name(name.as_str())?;
        }
        let get: Value = desc.get("get")?;
        if let Some(func) = get.into_function() {
            func.set_name(format!("get {name}"))?;
            let getter_desc = Object::new(ctx.clone())?;
            getter_desc.set("get", func)?;
            getter_desc.set("enumerable", false)?;
            getter_desc.set("configurable", true)?;
            crate::shape::define_property(&proto, &name, getter_desc)?;
        }
    }
    let ctor_names: Vec<String> = ctor_obj
        .own_keys::<String>(Filter::new().string())
        .collect::<Result<_>>()?;
    for name in ctor_names {
        if name == "prototype" || name == "length" || name == "name" {
            continue;
        }
        let Some(desc) = crate::shape::get_own_descriptor(&ctor_obj, &name)? else {
            continue;
        };
        let value: Value = desc.get("value")?;
        if let Some(func) = value.as_function() {
            func.set_name(name)?;
        }
    }
    ctor_obj.prop(PredefinedAtom::Prototype, Property::from(proto))?;
    Ok(())
}

fn copy_statics<'js>(original: &Constructor<'js>, wrapped: &Object<'js>) -> Result<()> {
    let names: Vec<String> = original
        .own_keys::<String>(Filter::new().string())
        .collect::<Result<_>>()?;
    for name in names {
        if name == "prototype" {
            continue;
        }
        if wrapped.has_own(&name)? {
            continue;
        }
        if let Some(desc) = crate::shape::get_own_descriptor(original, &name)? {
            crate::shape::define_property(wrapped, &name, desc)?;
        }
    }
    Ok(())
}

fn as_object_like<'js>(value: &Value<'js>) -> Option<Object<'js>> {
    value
        .as_function()
        .map(|func| func.clone().into_inner())
        .or_else(|| value.as_object().cloned())
}

#[rquickjs::methods(rename_all = "camelCase")]
impl PlainMonthDay {
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
        let object = item.as_object().expect("partial temporal object").clone();
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
        let display = match options_object(&ctx, options)? {
            None => DisplayCalendar::Auto,
            Some(object) => {
                match get_defined(&object, "calendarName")? {
                    None => DisplayCalendar::Auto,
                    Some(value) => option_display_calendar(&ctx, &value)?,
                }
            }
        };
        Ok(self.inner.to_ixdtf_string(display))
    }

    pub fn to_locale_string(&self) -> String { self.inner.to_ixdtf_string(DisplayCalendar::Auto) }

    #[qjs(rename = "toJSON")]
    pub fn to_json(&self) -> String { self.inner.to_ixdtf_string(DisplayCalendar::Auto) }

    pub fn value_of(&self, ctx: Ctx<'_>) -> Result<()> {
        Err(throw_value_of(&ctx, "Temporal.PlainMonthDay"))
    }

    #[qjs(prop, rename = PredefinedAtom::SymbolToStringTag, configurable)]
    pub fn to_string_tag() -> &'static str { "Temporal.PlainMonthDay" }
}

struct MonthDayBag {
    day:        Option<u8>,
    month:      Option<u8>,
    month_code: Option<String>,
    year:       Option<i32>,
}

impl MonthDayBag {
    fn is_empty(&self) -> bool {
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
    if item.is_object() {
        let object = item.as_object().expect("object").clone();
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
    let month_code = match get_defined(object, "monthCode")? {
        None => None,
        Some(value) => {
            let code = to_js_string(ctx, &value)?;
            reject_illformed_month_code(ctx, &code)?;
            Some(code)
        }
    };
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
    match get_defined(object, "calendar")? {
        None => Ok(Calendar::ISO),
        Some(value) => calendar_from_value(ctx, &value),
    }
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
    match calendar.0 {
        None => Ok(Calendar::ISO),
        Some(value) if value.is_undefined() => Ok(Calendar::ISO),
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

fn overflow_option<'js>(ctx: &Ctx<'js>, options: Opt<Value<'js>>) -> Result<Option<Overflow>> {
    let Some(object) = options_object(ctx, options)? else {
        return Ok(None);
    };
    match get_defined(&object, "overflow")? {
        None => Ok(None),
        Some(value) => option_overflow(ctx, &value).map(Some),
    }
}

fn option_overflow<'js>(ctx: &Ctx<'js>, value: &Value<'js>) -> Result<Overflow> {
    let name = to_js_string(ctx, value)?;
    Overflow::from_str(&name).map_err(|_| Exception::throw_range(ctx, "invalid overflow option"))
}

fn option_display_calendar<'js>(ctx: &Ctx<'js>, value: &Value<'js>) -> Result<DisplayCalendar> {
    let name = to_js_string(ctx, value)?;
    DisplayCalendar::from_str(&name)
        .map_err(|_| Exception::throw_range(ctx, "invalid calendarName option"))
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
