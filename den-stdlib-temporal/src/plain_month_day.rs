use std::str::FromStr;

use rquickjs::{
    Ctx, Exception, Function, JsLifetime, Object, Result, Value, atom::PredefinedAtom,
    class::{
        impl_::{ConstructorCreate, ConstructorCreator},
        Trace,
    },
    function::Constructor,
    object::Property,
    prelude::{Opt, This},
};
use temporal_rs::{
    Calendar, MonthCode,
    fields::CalendarFields,
    options::{DisplayCalendar, Overflow},
    partial::PartialDate,
};

use crate::{
    convert::{
        js_to_string, probe_class, reject_illformed_month_code, throw_temporal, throw_value_of,
        to_integer_with_truncation, to_plain_month_day, unwrap_temporal,
    },
    plain_date::PlainDate,
    plain_date_time::PlainDateTime,
    plain_time::PlainTime,
    plain_year_month::PlainYearMonth,
    zoned_date_time::ZonedDateTime,
};

#[derive(Trace, JsLifetime, Clone)]
#[rquickjs::class(rename = "PlainMonthDay", frozen)]
pub struct PlainMonthDay {
    #[qjs(skip_trace)]
    pub(crate) inner: temporal_rs::PlainMonthDay,
}

impl PlainMonthDay {
    pub(crate) fn wrap(inner: temporal_rs::PlainMonthDay) -> Self {
        Self { inner }
    }

    pub fn new<'js>(
        iso_month: Opt<Value<'js>>, iso_day: Opt<Value<'js>>, calendar: Opt<Value<'js>>,
        reference_iso_year: Opt<Value<'js>>, ctx: Ctx<'js>,
    ) -> Result<Self> {
        let month = ctor_truncated_u8(&ctx, iso_month)?;
        let day = ctor_truncated_u8(&ctx, iso_day)?;
        let calendar = ctor_calendar(&ctx, calendar)?;
        let reference_year = match reference_iso_year.0 {
            None => None,
            Some(value) if value.is_undefined() => None,
            Some(value) => {
                let integer = to_integer_with_truncation(&ctx, &value)?;
                Some(
                    i32::try_from(integer)
                        .map_err(|_| Exception::throw_range(&ctx, "integer is out of range"))?,
                )
            }
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
        let constr = Constructor::new_class::<PlainMonthDay, _, _>(ctx.clone(), PlainMonthDay::new)?;
        let func: &Function = constr.as_ref();
        func.set_length(2)?;
        let from = Function::new(ctx.clone(), PlainMonthDay::from)?.with_name("from")?;
        let object: &Object = func.as_ref();
        object.prop("from", Property::from(from).writable().configurable())?;
        // lib.rs replaces the constructor with a NewTarget wrapper whose
        // [[Prototype]] is this original. Copy own statics onto that wrapper
        // and restore Function.prototype when it writes `.constructor`.
        let install: Function = ctx.eval(
            r#"(original) => {
  const proto = original.prototype;
  const copyStatics = (wrapped) => {
    for (const name of Object.getOwnPropertyNames(original)) {
      if (name === "prototype") continue;
      if (Object.prototype.hasOwnProperty.call(wrapped, name)) continue;
      const desc = Object.getOwnPropertyDescriptor(original, name);
      if (desc) Object.defineProperty(wrapped, name, desc);
    }
  };
  Object.defineProperty(proto, "constructor", {
    configurable: true,
    enumerable: false,
    get() { return original; },
    set(wrapped) {
      copyStatics(wrapped);
      Object.defineProperty(proto, "constructor", {
        value: wrapped,
        writable: true,
        enumerable: false,
        configurable: true,
      });
    },
  });
  const finish = (ctor) => {
    if (!ctor) return;
    copyStatics(ctor);
    Object.setPrototypeOf(ctor, Function.prototype);
    const fixName = (fn, name) => {
      if (typeof fn !== "function") return;
      Object.defineProperty(fn, "name", {
        value: name,
        writable: false,
        enumerable: false,
        configurable: true,
      });
    };
    const proto = ctor.prototype;
    for (const name of Object.getOwnPropertyNames(proto)) {
      if (name === "constructor") continue;
      const desc = Object.getOwnPropertyDescriptor(proto, name);
      if (!desc) continue;
      if (typeof desc.value === "function") {
        fixName(desc.value, name);
      }
      if (typeof desc.get === "function") {
        fixName(desc.get, "get " + name);
        Object.defineProperty(proto, name, {
          get: desc.get,
          enumerable: false,
          configurable: true,
        });
      }
    }
    for (const name of Object.getOwnPropertyNames(ctor)) {
      if (name === "prototype" || name === "length" || name === "name") continue;
      const desc = Object.getOwnPropertyDescriptor(ctor, name);
      if (desc && typeof desc.value === "function") {
        fixName(desc.value, name);
      }
    }
    Object.defineProperty(ctor, "prototype", {
      value: proto,
      writable: false,
      enumerable: false,
      configurable: false,
    });
  };
  const existing = Object.getOwnPropertyDescriptor(globalThis, "Temporal");
  Object.defineProperty(globalThis, "Temporal", {
    configurable: true,
    enumerable: false,
    get() { return existing && existing.get ? existing.get.call(globalThis) : existing && existing.value; },
    set(ns) {
      finish(ns && ns.PlainMonthDay);
      Object.defineProperty(globalThis, "Temporal", {
        value: ns,
        writable: true,
        enumerable: false,
        configurable: true,
      });
    },
  });
}"#,
        )?;
        install.call::<_, ()>((constr.clone(),))?;
        Ok(Some(constr))
    }
}

#[rquickjs::methods(rename_all = "camelCase")]
impl PlainMonthDay {

    #[qjs(get, configurable)]
    pub fn calendar_id(&self) -> &'static str {
        self.inner.calendar_id()
    }

    #[qjs(get, configurable)]
    pub fn month_code(&self) -> String {
        self.inner.month_code().as_str().to_string()
    }

    #[qjs(get, configurable)]
    pub fn day(&self) -> u8 {
        self.inner.day()
    }

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
        let object = required_object(&ctx, &item, "toPlainDate() requires an object")?;
        let Some(year) = get_defined(&object, "year")? else {
            return Err(Exception::throw_type(&ctx, "year is required"));
        };
        let fields = CalendarFields::new().with_year(field_to_i32(&ctx, &year)?);
        unwrap_temporal(&ctx, self.inner.to_plain_date(Some(fields))).map(PlainDate::wrap)
    }

    pub fn to_string<'js>(&self, options: Opt<Value<'js>>, ctx: Ctx<'js>) -> Result<String> {
        let display = match options_object(&ctx, options)? {
            None => DisplayCalendar::Auto,
            Some(object) => {
                let value: Value = object.get("calendarName")?;
                if value.is_undefined() {
                    DisplayCalendar::Auto
                } else {
                    option_display_calendar(&ctx, &value)?
                }
            }
        };
        Ok(self.inner.to_ixdtf_string(display))
    }

    pub fn to_locale_string(&self) -> String {
        self.inner.to_ixdtf_string(DisplayCalendar::Auto)
    }

    #[qjs(rename = "toJSON")]
    pub fn to_json(&self) -> String {
        self.inner.to_ixdtf_string(DisplayCalendar::Auto)
    }

    pub fn value_of(&self, ctx: Ctx<'_>) -> Result<()> {
        Err(throw_value_of(&ctx, "Temporal.PlainMonthDay"))
    }

    #[qjs(prop, rename = PredefinedAtom::SymbolToStringTag, configurable)]
    pub fn to_string_tag() -> &'static str {
        "Temporal.PlainMonthDay"
    }
}

struct MonthDayBag {
    day: Option<u8>,
    month: Option<u8>,
    month_code: Option<String>,
    year: Option<i32>,
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
        Some(value) => Some(field_to_i32(ctx, &value)?),
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
    if let Some(calendar) = calendar_from_temporal(ctx, value) {
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

fn calendar_from_temporal<'js>(ctx: &Ctx<'js>, value: &Value<'js>) -> Option<Calendar> {
    if let Some(date) = probe_class::<PlainDate>(ctx, value) {
        return Some(date.inner.calendar().clone());
    }
    if let Some(date_time) = probe_class::<PlainDateTime>(ctx, value) {
        return Some(date_time.inner.calendar().clone());
    }
    if let Some(month_day) = probe_class::<PlainMonthDay>(ctx, value) {
        return Some(month_day.inner.calendar().clone());
    }
    if let Some(year_month) = probe_class::<PlainYearMonth>(ctx, value) {
        return Some(year_month.inner.calendar().clone());
    }
    if let Some(zoned) = probe_class::<ZonedDateTime>(ctx, value) {
        return Some(zoned.inner.calendar().clone());
    }
    None
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

fn ctor_truncated_u8<'js>(ctx: &Ctx<'js>, value: Opt<Value<'js>>) -> Result<u8> {
    let value = match value.0 {
        None => Value::new_undefined(ctx.clone()),
        Some(value) => value,
    };
    let integer = to_integer_with_truncation(ctx, &value)?;
    u8::try_from(integer).map_err(|_| Exception::throw_range(ctx, "integer is out of range"))
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

fn field_to_i32<'js>(ctx: &Ctx<'js>, value: &Value<'js>) -> Result<i32> {
    let integer = to_integer_with_truncation(ctx, value)?;
    i32::try_from(integer).map_err(|_| Exception::throw_range(ctx, "integer is out of range"))
}

fn overflow_option<'js>(ctx: &Ctx<'js>, options: Opt<Value<'js>>) -> Result<Option<Overflow>> {
    let Some(object) = options_object(ctx, options)? else {
        return Ok(None);
    };
    let value: Value = object.get("overflow")?;
    if value.is_undefined() {
        return Ok(None);
    }
    option_overflow(ctx, &value).map(Some)
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
    if probe_class::<PlainDate>(ctx, value).is_some()
        || probe_class::<PlainDateTime>(ctx, value).is_some()
        || probe_class::<PlainMonthDay>(ctx, value).is_some()
        || probe_class::<PlainTime>(ctx, value).is_some()
        || probe_class::<PlainYearMonth>(ctx, value).is_some()
        || probe_class::<ZonedDateTime>(ctx, value).is_some()
    {
        return Ok(false);
    }
    if get_defined(object, "calendar")?.is_some() || get_defined(object, "timeZone")?.is_some() {
        return Ok(false);
    }
    Ok(true)
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
        .map(Some)
        .ok_or_else(|| Exception::throw_type(ctx, "options must be an object"))
}

fn required_object<'js>(ctx: &Ctx<'js>, value: &Value<'js>, message: &str) -> Result<Object<'js>> {
    value
        .as_object()
        .cloned()
        .ok_or_else(|| Exception::throw_type(ctx, message))
}

fn get_defined<'js>(object: &Object<'js>, key: &str) -> Result<Option<Value<'js>>> {
    let value: Value = object.get(key)?;
    if value.is_undefined() {
        Ok(None)
    } else {
        Ok(Some(value))
    }
}
