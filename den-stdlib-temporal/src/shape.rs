//! Stamp Temporal constructors, statics, and prototype methods with test262
//! `name` / `length` / property descriptors, and RequireInternalSlot branding.
//!
//! rquickjs installs methods as non-configurable own properties, so this
//! rebuilds each interface as a constructor (writable:false `prototype`) and
//! rehomes values that the Rust class still stamps onto the original proto.

use den_util::ObjectExt as _;
use rquickjs::{
    Constructor, Ctx, Exception, Filter, Function, IntoJs as _, Object, Result, Symbol, Value,
    atom::PredefinedAtom,
    function::{Args, Opt, Rest, This},
    object::{Accessor, Property},
};

use crate::convert::{
    get_defined, js_to_string, to_integer_if_integral, to_integer_with_truncation,
};

const INTERFACES: [&str; 8] = [
    "Instant",
    "Duration",
    "PlainDate",
    "PlainTime",
    "PlainDateTime",
    "PlainYearMonth",
    "PlainMonthDay",
    "ZonedDateTime",
];

struct Brand<'js> {
    name: &'static str,
    original: Constructor<'js>,
    wrapped: Constructor<'js>,
    original_proto: Object<'js>,
    proto: Object<'js>,
}

/// WebIDL-shaped Temporal namespace plus per-interface method metadata.
pub fn define_interface_shape<'js>(
    ctx: &Ctx<'js>, namespace: Object<'js>, now: Object<'js>,
) -> Result<()> {
    let brands = INTERFACES
        .into_iter()
        .map(|name| {
            let original: Constructor = namespace.get(name)?;
            wrap_constructor(ctx, name, original)
        })
        .collect::<Result<Vec<_>>>()?;

    let originals = Object::new(ctx.clone())?;
    for brand in &brands {
        originals.set(brand.name, brand.original.clone())?;
        install_interface(ctx, brand)?;
        tag(&brand.proto, &format!("Temporal.{}", brand.name))?;
        namespace.set(brand.name, brand.wrapped.clone())?;
    }
    namespace.prop(
        interned_symbol(ctx, ORIGINALS_KEY)?,
        Property::from(originals),
    )?;

    wrap_now(ctx, &now)?;
    tag(&now, "Temporal.Now")?;
    namespace.set("Now", now)?;

    let names: Vec<String> = namespace.keys::<String>().collect::<Result<_>>()?;
    for name in names {
        hide(&namespace, &name)?;
    }
    tag(&namespace, "Temporal")?;
    ctx.globals().prop(
        "Temporal",
        Property::from(namespace).writable().configurable(),
    )?;
    Ok(())
}

fn constructor_length(name: &str) -> usize {
    match name {
        "Instant" => 1,
        "PlainDate" | "PlainDateTime" => 3,
        "PlainYearMonth" | "PlainMonthDay" | "ZonedDateTime" => 2,
        _ => 0,
    }
}

fn static_length(name: &str) -> Option<usize> {
    match name {
        "from" | "fromEpochNanoseconds" | "fromEpochMilliseconds" => Some(1),
        "compare" => Some(2),
        _ => None,
    }
}

fn proto_length(type_name: &str, method_name: &str) -> usize {
    match (type_name, method_name) {
        ("PlainYearMonth" | "PlainMonthDay", "toPlainDate")
        | (
            _,
            "add"
            | "equals"
            | "getTimeZoneTransition"
            | "round"
            | "since"
            | "subtract"
            | "toZonedDateTime"
            | "toZonedDateTimeISO"
            | "total"
            | "until"
            | "with"
            | "withCalendar"
            | "withTimeZone",
        ) => 1,
        _ => 0,
    }
}

fn rename(key: &str) -> &str {
    match key {
        "toJson" | "to_json" => "toJSON",
        "toZonedDateTimeIso" => "toZonedDateTimeISO",
        "to_string" => "toString",
        "value_of" => "valueOf",
        _ => key,
    }
}

fn required_methods(type_name: &str) -> &'static [&'static str] {
    match type_name {
        "Instant" => &[
            "add",
            "equals",
            "round",
            "since",
            "subtract",
            "toJSON",
            "toLocaleString",
            "toString",
            "toZonedDateTimeISO",
            "until",
            "valueOf",
        ],
        "Duration" => &[
            "abs",
            "add",
            "negated",
            "round",
            "subtract",
            "toJSON",
            "toLocaleString",
            "toString",
            "total",
            "valueOf",
            "with",
        ],
        "PlainDate" => &[
            "add",
            "equals",
            "since",
            "subtract",
            "toJSON",
            "toLocaleString",
            "toPlainDateTime",
            "toPlainMonthDay",
            "toPlainYearMonth",
            "toString",
            "toZonedDateTime",
            "until",
            "valueOf",
            "with",
            "withCalendar",
        ],
        "PlainTime" => &[
            "add",
            "equals",
            "round",
            "since",
            "subtract",
            "toJSON",
            "toLocaleString",
            "toString",
            "until",
            "valueOf",
            "with",
        ],
        "PlainDateTime" => &[
            "add",
            "equals",
            "round",
            "since",
            "subtract",
            "toJSON",
            "toLocaleString",
            "toPlainDate",
            "toPlainTime",
            "toString",
            "toZonedDateTime",
            "until",
            "valueOf",
            "with",
            "withCalendar",
            "withPlainTime",
        ],
        "PlainYearMonth" => &[
            "add",
            "equals",
            "since",
            "subtract",
            "toJSON",
            "toLocaleString",
            "toPlainDate",
            "toString",
            "until",
            "valueOf",
            "with",
        ],
        "PlainMonthDay" => &[
            "equals",
            "toJSON",
            "toLocaleString",
            "toPlainDate",
            "toString",
            "valueOf",
            "with",
        ],
        "ZonedDateTime" => &[
            "add",
            "equals",
            "getTimeZoneTransition",
            "round",
            "since",
            "startOfDay",
            "subtract",
            "toInstant",
            "toJSON",
            "toLocaleString",
            "toPlainDate",
            "toPlainDateTime",
            "toPlainTime",
            "toString",
            "until",
            "valueOf",
            "with",
            "withCalendar",
            "withPlainTime",
            "withTimeZone",
        ],
        _ => &[],
    }
}

fn required_getters(type_name: &str) -> &'static [&'static str] {
    match type_name {
        "Instant" => &["epochNanoseconds", "epochMilliseconds"],
        "Duration" => &[
            "years",
            "months",
            "weeks",
            "days",
            "hours",
            "minutes",
            "seconds",
            "milliseconds",
            "microseconds",
            "nanoseconds",
            "sign",
            "blank",
        ],
        "PlainDate" => &[
            "calendarId",
            "era",
            "eraYear",
            "year",
            "month",
            "monthCode",
            "day",
            "dayOfWeek",
            "dayOfYear",
            "weekOfYear",
            "yearOfWeek",
            "daysInWeek",
            "daysInMonth",
            "daysInYear",
            "monthsInYear",
            "inLeapYear",
        ],
        "PlainTime" => &[
            "hour",
            "minute",
            "second",
            "millisecond",
            "microsecond",
            "nanosecond",
        ],
        "PlainDateTime" => &[
            "calendarId",
            "era",
            "eraYear",
            "year",
            "month",
            "monthCode",
            "day",
            "hour",
            "minute",
            "second",
            "millisecond",
            "microsecond",
            "nanosecond",
            "dayOfWeek",
            "dayOfYear",
            "weekOfYear",
            "yearOfWeek",
            "daysInWeek",
            "daysInMonth",
            "daysInYear",
            "monthsInYear",
            "inLeapYear",
        ],
        "PlainYearMonth" => &[
            "calendarId",
            "era",
            "eraYear",
            "year",
            "month",
            "monthCode",
            "daysInYear",
            "daysInMonth",
            "monthsInYear",
            "inLeapYear",
        ],
        "PlainMonthDay" => &["calendarId", "monthCode", "day"],
        "ZonedDateTime" => &[
            "calendarId",
            "timeZoneId",
            "era",
            "eraYear",
            "year",
            "month",
            "monthCode",
            "day",
            "hour",
            "minute",
            "second",
            "millisecond",
            "microsecond",
            "nanosecond",
            "epochNanoseconds",
            "epochMilliseconds",
            "dayOfWeek",
            "dayOfYear",
            "weekOfYear",
            "yearOfWeek",
            "daysInWeek",
            "daysInMonth",
            "daysInYear",
            "monthsInYear",
            "inLeapYear",
            "offset",
            "offsetNanoseconds",
            "hoursInDay",
        ],
        _ => &[],
    }
}

fn required_statics(type_name: &str) -> &'static [&'static str] {
    match type_name {
        "Instant" => &[
            "from",
            "compare",
            "fromEpochNanoseconds",
            "fromEpochMilliseconds",
        ],
        "PlainMonthDay" => &["from"],
        "Duration" | "PlainDate" | "PlainTime" | "PlainDateTime" | "PlainYearMonth"
        | "ZonedDateTime" => &["from", "compare"],
        _ => &[],
    }
}

fn with_fields(type_name: &str) -> Option<&'static [&'static str]> {
    match type_name {
        "Duration" => Some(&[
            "years",
            "months",
            "weeks",
            "days",
            "hours",
            "minutes",
            "seconds",
            "milliseconds",
            "microseconds",
            "nanoseconds",
        ]),
        "PlainDate" | "PlainMonthDay" => Some(&["year", "month", "monthCode", "day"]),
        "PlainTime" => Some(&[
            "hour",
            "minute",
            "second",
            "millisecond",
            "microsecond",
            "nanosecond",
        ]),
        "PlainDateTime" => Some(&[
            "year",
            "month",
            "monthCode",
            "day",
            "hour",
            "minute",
            "second",
            "millisecond",
            "microsecond",
            "nanosecond",
        ]),
        "PlainYearMonth" => Some(&["year", "month", "monthCode"]),
        "ZonedDateTime" => Some(&[
            "year",
            "month",
            "monthCode",
            "day",
            "hour",
            "minute",
            "second",
            "millisecond",
            "microsecond",
            "nanosecond",
            "offset",
        ]),
        _ => None,
    }
}

fn wrap_constructor<'js>(
    ctx: &Ctx<'js>, name: &'static str, original: Constructor<'js>,
) -> Result<Brand<'js>> {
    let original_proto: Object = original.get(PredefinedAtom::Prototype)?;
    let proto = Object::new(ctx.clone())?;
    let wrapped = Constructor::new_prototype(
        ctx,
        proto.clone(),
        move |ctx: Ctx<'js>,
              this: This<Value<'js>>,
              args: Rest<Value<'js>>|
              -> Result<Value<'js>> {
            if !this.0.is_function() {
                return Err(Exception::throw_type(
                    &ctx,
                    "class constructors must be invoked with 'new'",
                ));
            }
            let original = original_constructor(&ctx, name)?;
            let original_proto: Object = original.get(PredefinedAtom::Prototype)?;
            let new_target = this.0;
            let instance = construct_with_new_target(&original, &new_target, args.0)?;
            if let Some(object) = instance.as_object() {
                let current = object.get_prototype();
                if current.as_ref() == Some(&original_proto) && new_target != *original.as_value() {
                    let ctor_obj = new_target
                        .as_function()
                        .map(|func| func.clone().into_inner())
                        .or_else(|| new_target.as_object().cloned());
                    if let Some(desired) = ctor_obj
                        .and_then(|ctor| ctor.get::<_, Value>(PredefinedAtom::Prototype).ok())
                        .and_then(Value::into_object)
                    {
                        object.set_prototype(Some(&desired))?;
                    }
                }
            }
            Ok(instance)
        },
    )?;
    wrapped.set_name(name)?;
    wrapped.set_length(constructor_length(name))?;
    Ok(Brand {
        name,
        original,
        wrapped,
        original_proto,
        proto,
    })
}

fn install_interface<'js>(ctx: &Ctx<'js>, brand: &Brand<'js>) -> Result<()> {
    let original_obj: &Object = &brand.original;
    let keys: Vec<String> = original_obj
        .own_keys::<String>(Filter::new().string())
        .collect::<Result<_>>()?;
    for key in keys {
        if key == "prototype" || key == "name" || key == "length" {
            continue;
        }
        if own_function(&brand.original, &key)?.is_none() {
            continue;
        }
        let spec_name = rename(&key);
        install_static(brand, spec_name, &key)?;
    }
    for spec_name in required_statics(brand.name) {
        if brand.wrapped.has_own(spec_name)? {
            continue;
        }
        let lowered = uncapitalize(spec_name);
        let found_key = if own_function(&brand.original, spec_name)?.is_some() {
            spec_name
        } else if own_function(&brand.original, &lowered)?.is_some() {
            lowered.as_str()
        } else {
            continue;
        };
        install_static(brand, spec_name, found_key)?;
    }

    let proto_keys: Vec<String> = brand
        .original_proto
        .own_keys::<String>(Filter::new().string())
        .collect::<Result<_>>()?;
    for key in proto_keys {
        if key == "constructor" {
            continue;
        }
        let spec_name = rename(&key).to_owned();
        if own_getter(&brand.original_proto, &key)?.is_some() {
            install_getter(brand, &spec_name, &key)?;
        } else if own_function(&brand.original_proto, &key)?.is_some() {
            install_method(
                brand,
                &spec_name,
                &key,
                proto_length(brand.name, &spec_name),
            )?;
        }
    }

    let object_proto: Object = {
        let object: Object = ctx.globals().get(PredefinedAtom::Object)?;
        object.get(PredefinedAtom::Prototype)?
    };
    let object_to_string: Function = object_proto.get("toString")?;
    let object_value_of: Function = object_proto.get("valueOf")?;

    let rust_to_string = match own_function(&brand.original_proto, "toString")? {
        Some(func) => Some(func),
        None => own_function(&brand.original_proto, "to_string")?,
    };
    let to_string_key = if own_function(&brand.original_proto, "toString")?.is_some() {
        "toString"
    } else {
        "to_string"
    };
    if rust_to_string
        .as_ref()
        .is_some_and(|func| *func != object_to_string)
    {
        install_method(brand, "toString", to_string_key, 0)?;
    }
    let rust_value_of = match own_function(&brand.original_proto, "valueOf")? {
        Some(func) => Some(func),
        None => own_function(&brand.original_proto, "value_of")?,
    };
    let value_of_key = if own_function(&brand.original_proto, "valueOf")?.is_some() {
        "valueOf"
    } else {
        "value_of"
    };
    if rust_value_of.is_some_and(|func| func != object_value_of) {
        install_method(brand, "valueOf", value_of_key, 0)?;
    }

    let proto_to_string = own_function(&brand.proto, "toString")?;
    let to_string_source = if proto_to_string
        .as_ref()
        .is_some_and(|func| *func != object_to_string)
    {
        match own_function(&brand.original_proto, "toString")? {
            Some(func) => Some(func),
            None => own_function(&brand.original_proto, "to_string")?.or(rust_to_string),
        }
    } else {
        rust_to_string
    };
    if to_string_source.is_some_and(|func| func != object_to_string) {
        install_method(brand, "toLocaleString", to_string_key, 0)?;
    }

    for spec_name in required_methods(brand.name) {
        install_stub_method(brand, spec_name, proto_length(brand.name, spec_name))?;
    }
    install_iso_with(brand)?;
    install_to_plain_date(brand)?;
    install_iso_round(brand)?;
    for spec_name in required_getters(brand.name) {
        match own_getter(&brand.original_proto, spec_name)? {
            Some(_) if !brand.proto.has_own(spec_name)? => {
                install_getter(brand, spec_name, spec_name)?;
            }
            _ => install_stub_getter(brand, spec_name)?,
        }
    }
    Ok(())
}

fn install_static(brand: &Brand<'_>, spec_name: &str, original_key: &str) -> Result<()> {
    let original_fn = own_function(&brand.original, original_key)?
        .or_else(|| own_function(&brand.original, spec_name).ok().flatten())
        .ok_or_else(|| Exception::throw_internal(brand.original.ctx(), "static method missing"))?;
    let length = match static_length(spec_name) {
        Some(length) => length,
        None => function_length(&original_fn)?,
    };
    let type_name = brand.name;
    let lookup_key = original_key.to_owned();
    let fn_ = if spec_name == "from" {
        make_from(brand.original.ctx(), type_name)?
    } else {
        make_fn(
            brand.original.ctx(),
            spec_name,
            length,
            move |ctx, _, args| {
                let original = original_constructor(&ctx, type_name)?;
                let original_fn = own_function(&original, &lookup_key)?
                    .ok_or_else(|| Exception::throw_type(&ctx, "static method missing"))?;
                rehome(
                    &ctx,
                    call_this(&original_fn, original.as_value().clone(), &args)?,
                )
            },
        )?
    };
    define_data(&brand.wrapped, spec_name, fn_)
}

fn make_from<'js>(ctx: &Ctx<'js>, type_name: &'static str) -> Result<Function<'js>> {
    make_fn(ctx, "from", 1, move |ctx, _, args| {
        let first = arg_at(&ctx, &args, 0);
        let second = arg_at(&ctx, &args, 1);
        let original = original_constructor(&ctx, type_name)?;
        let original_from = own_function(&original, "from")?
            .ok_or_else(|| Exception::throw_type(&ctx, "from is not implemented"))?;
        rehome(
            &ctx,
            call_this(
                &original_from,
                original.as_value().clone(),
                &[first, second],
            )?,
        )
    })
}

fn install_method(
    brand: &Brand<'_>, spec_name: &str, original_key: &str, length: usize,
) -> Result<()> {
    let ignore_args = spec_name == "toLocaleString";
    let type_name = brand.name;
    let spec = spec_name.to_owned();
    let lookup_key = original_key.to_owned();
    let fn_ = make_fn(
        brand.proto.ctx(),
        spec_name,
        length,
        move |ctx, this, args| {
            if !is_brand(&ctx, &this, type_name)? {
                return Err(Exception::throw_type(
                    &ctx,
                    &format!("{spec} called on incompatible receiver"),
                ));
            }
            require_options_bag(&ctx, &spec, &args)?;
            let proto = original_prototype(&ctx, type_name)?;
            let original_fn = own_function(&proto, &lookup_key)?
                .or_else(|| own_function(&proto, "to_string").ok().flatten())
                .ok_or_else(|| Exception::throw_type(&ctx, "method missing"))?;
            let applied = if ignore_args {
                call_this(&original_fn, this, &[])?
            } else {
                call_this(&original_fn, this, &args)?
            };
            rehome(&ctx, applied)
        },
    )?;
    define_data(&brand.proto, spec_name, fn_)
}

fn install_getter<'js>(brand: &Brand<'js>, spec_name: &str, original_key: &str) -> Result<()> {
    let type_name = brand.name;
    let getter_name = format!("get {spec_name}");
    let lookup_key = original_key.to_owned();
    define_named_getter(
        &brand.proto,
        spec_name,
        move |ctx: Ctx<'js>, this: This<Value<'js>>| -> Result<Value<'js>> {
            if !is_brand(&ctx, &this.0, type_name)? {
                return Err(Exception::throw_type(
                    &ctx,
                    &format!("{getter_name} called on incompatible receiver"),
                ));
            }
            let proto = original_prototype(&ctx, type_name)?;
            let original_get = own_getter(&proto, &lookup_key)?
                .ok_or_else(|| Exception::throw_type(&ctx, "getter missing"))?;
            rehome(&ctx, call_this(&original_get, this.0, &[])?)
        },
    )
}

fn install_stub_method(brand: &Brand<'_>, spec_name: &str, length: usize) -> Result<()> {
    if get_own_descriptor(&brand.proto, spec_name)?.is_some() {
        return Ok(());
    }
    let type_name = brand.name;
    let spec = spec_name.to_owned();
    let fn_ = make_fn(brand.proto.ctx(), spec_name, length, move |ctx, this, _| {
        if !is_brand(&ctx, &this, type_name)? {
            return Err(Exception::throw_type(
                &ctx,
                &format!("{spec} called on incompatible receiver"),
            ));
        }
        Err(Exception::throw_type(
            &ctx,
            &format!("{spec} is not implemented"),
        ))
    })?;
    define_data(&brand.proto, spec_name, fn_)
}

fn install_stub_getter<'js>(brand: &Brand<'js>, spec_name: &str) -> Result<()> {
    if get_own_descriptor(&brand.proto, spec_name)?.is_some() {
        return Ok(());
    }
    let type_name = brand.name;
    let getter_name = format!("get {spec_name}");
    let spec = spec_name.to_owned();
    define_named_getter(
        &brand.proto,
        spec_name,
        move |ctx: Ctx<'js>, this: This<Value<'js>>| {
            if !is_brand(&ctx, &this.0, type_name)? {
                return Err(Exception::throw_type(
                    &ctx,
                    &format!("{getter_name} called on incompatible receiver"),
                ));
            }
            if spec == "era" || spec == "eraYear" {
                return Ok(Value::new_undefined(ctx.clone()));
            }
            Err(Exception::throw_type(
                &ctx,
                &format!("{spec} is not implemented"),
            ))
        },
    )
}

fn install_iso_with(brand: &Brand<'_>) -> Result<()> {
    if own_function(&brand.original_proto, "with")?.is_some() {
        return Ok(());
    }
    let Some(field_keys) = with_fields(brand.name) else {
        return Ok(());
    };
    let type_name = brand.name;
    let fn_ = make_fn(brand.proto.ctx(), "with", 1, move |ctx, this, args| {
        if !is_brand(&ctx, &this, type_name)? {
            return Err(Exception::throw_type(
                &ctx,
                "with called on incompatible receiver",
            ));
        }
        let fields = arg_at(&ctx, &args, 0);
        let options = args.get(1).cloned();
        if !is_js_object(&fields) {
            return Err(Exception::throw_type(&ctx, "argument must be an object"));
        }
        let fields_obj = js_object(&fields)?;
        if type_name != "Duration"
            && (get_defined(&fields_obj, "calendar")?.is_some()
                || get_defined(&fields_obj, "timeZone")?.is_some())
        {
            return Err(Exception::throw_type(
                &ctx,
                "with() does not accept calendar or timeZone",
            ));
        }
        let partial = Object::new(ctx.clone())?;
        let mut present = false;
        for key in field_keys {
            let Some(value) = get_defined(&fields_obj, key)? else {
                continue;
            };
            present = true;
            if *key == "monthCode" || *key == "offset" {
                partial.set(*key, js_to_string(&ctx, &value)?)?;
            } else if type_name == "Duration" {
                partial.set(*key, to_integer_if_integral(&ctx, &value)? as f64)?;
            } else {
                partial.set(*key, to_integer_with_truncation(&ctx, &value)? as f64)?;
            }
        }
        if !present {
            return Err(Exception::throw_type(&ctx, "invalid with() argument"));
        }
        let result = construct_with(&ctx, type_name, &this, &partial)?;
        if type_name != "Duration" {
            to_temporal_overflow(&ctx, options.as_ref())?;
        }
        Ok(result)
    })?;
    define_data(&brand.proto, "with", fn_)
}

fn install_to_plain_date(brand: &Brand<'_>) -> Result<()> {
    if brand.name != "PlainYearMonth" && brand.name != "PlainMonthDay" {
        return Ok(());
    }
    if own_function(&brand.original_proto, "toPlainDate")?.is_some()
        || own_function(&brand.original_proto, "to_plain_date")?.is_some()
    {
        return Ok(());
    }
    let type_name = brand.name;
    let fn_ = make_fn(
        brand.proto.ctx(),
        "toPlainDate",
        1,
        move |ctx, this, args| {
            if !is_brand(&ctx, &this, type_name)? {
                return Err(Exception::throw_type(
                    &ctx,
                    "toPlainDate called on incompatible receiver",
                ));
            }
            let item = arg_at(&ctx, &args, 0);
            if !is_js_object(&item) {
                return Err(Exception::throw_type(&ctx, "argument must be an object"));
            }
            let item_obj = js_object(&item)?;
            let self_obj = js_object(&this)?;
            let plain_date = wrapped_constructor(&ctx, "PlainDate")?;
            if type_name == "PlainYearMonth" {
                let Some(day) = get_defined(&item_obj, "day")? else {
                    return Err(Exception::throw_type(&ctx, "day is required"));
                };
                let truncated = to_integer_with_truncation(&ctx, &day)? as f64;
                return rehome(
                    &ctx,
                    construct(
                        &plain_date,
                        [
                            self_obj.get("year")?,
                            self_obj.get("month")?,
                            truncated.into_js(&ctx)?,
                            self_obj.get("calendarId")?,
                        ],
                    )?,
                );
            }
            let Some(year) = get_defined(&item_obj, "year")? else {
                return Err(Exception::throw_type(&ctx, "year is required"));
            };
            let month = parse_month_code(&ctx, &self_obj.get("monthCode")?)?;
            rehome(
                &ctx,
                construct(
                    &plain_date,
                    [
                        (to_integer_with_truncation(&ctx, &year)? as f64).into_js(&ctx)?,
                        month.into_js(&ctx)?,
                        self_obj.get("day")?,
                        self_obj.get("calendarId")?,
                    ],
                )?,
            )
        },
    )?;
    define_data(&brand.proto, "toPlainDate", fn_)
}

fn install_iso_round(brand: &Brand<'_>) -> Result<()> {
    if !matches!(brand.name, "PlainDateTime" | "PlainTime" | "ZonedDateTime") {
        return Ok(());
    }
    if own_function(&brand.original_proto, "round")?.is_some() {
        return Ok(());
    }
    let type_name = brand.name;
    let fn_ = make_fn(brand.proto.ctx(), "round", 1, move |ctx, this, args| {
        if !is_brand(&ctx, &this, type_name)? {
            return Err(Exception::throw_type(
                &ctx,
                "round called on incompatible receiver",
            ));
        }
        require_options_bag(&ctx, "round", &args)?;
        let options = arg_at(&ctx, &args, 0);
        let instant = wrapped_constructor(&ctx, "Instant")?;
        let from_epoch: Function = instant.get("fromEpochNanoseconds")?;
        let self_obj = js_object(&this)?;
        if type_name == "ZonedDateTime" {
            let rounded: Object = from_epoch
                .call::<_, Value>((self_obj.get::<_, Value>("epochNanoseconds")?,))?
                .into_object()
                .ok_or_else(|| Exception::throw_type(&ctx, "expected Instant"))?;
            let round: Function = rounded.get("round")?;
            let rounded: Object = call_this(&round, rounded.as_value().clone(), &[options])?
                .into_object()
                .ok_or_else(|| Exception::throw_type(&ctx, "expected Instant"))?;
            let to_zoned: Function = rounded.get("toZonedDateTimeISO")?;
            return rehome(
                &ctx,
                call_this(
                    &to_zoned,
                    rounded.into_value(),
                    &[self_obj.get("timeZoneId")?],
                )?,
            );
        }
        if type_name == "PlainDateTime" {
            let to_zoned: Function = self_obj.get("toZonedDateTime")?;
            let zoned: Object = call_this(&to_zoned, this.clone(), &["UTC".into_js(&ctx)?])?
                .into_object()
                .ok_or_else(|| Exception::throw_type(&ctx, "expected ZonedDateTime"))?;
            let rounded: Object = from_epoch
                .call::<_, Value>((zoned.get::<_, Value>("epochNanoseconds")?,))?
                .into_object()
                .ok_or_else(|| Exception::throw_type(&ctx, "expected Instant"))?;
            let round: Function = rounded.get("round")?;
            let rounded: Object = call_this(&round, rounded.as_value().clone(), &[options])?
                .into_object()
                .ok_or_else(|| Exception::throw_type(&ctx, "expected Instant"))?;
            let to_zoned: Function = rounded.get("toZonedDateTimeISO")?;
            let zoned: Object =
                call_this(&to_zoned, rounded.into_value(), &["UTC".into_js(&ctx)?])?
                    .into_object()
                    .ok_or_else(|| Exception::throw_type(&ctx, "expected ZonedDateTime"))?;
            let to_plain: Function = zoned.get("toPlainDateTime")?;
            return rehome(&ctx, call_this(&to_plain, zoned.into_value(), &[])?);
        }
        let zoned_date_time = wrapped_constructor(&ctx, "ZonedDateTime")?;
        let plain_time = wrapped_constructor(&ctx, "PlainTime")?;
        let bag = Object::new(ctx.clone())?;
        bag.set("year", 1970)?;
        bag.set("month", 1)?;
        bag.set("day", 1)?;
        bag.set("hour", self_obj.get::<_, Value>("hour")?)?;
        bag.set("minute", self_obj.get::<_, Value>("minute")?)?;
        bag.set("second", self_obj.get::<_, Value>("second")?)?;
        bag.set("millisecond", self_obj.get::<_, Value>("millisecond")?)?;
        bag.set("microsecond", self_obj.get::<_, Value>("microsecond")?)?;
        bag.set("nanosecond", self_obj.get::<_, Value>("nanosecond")?)?;
        bag.set("timeZone", "UTC")?;
        let from: Function = zoned_date_time.get("from")?;
        let zoned: Object = from
            .call::<_, Value>((bag,))?
            .into_object()
            .ok_or_else(|| Exception::throw_type(&ctx, "expected ZonedDateTime"))?;
        let rounded: Object = from_epoch
            .call::<_, Value>((zoned.get::<_, Value>("epochNanoseconds")?,))?
            .into_object()
            .ok_or_else(|| Exception::throw_type(&ctx, "expected Instant"))?;
        let round: Function = rounded.get("round")?;
        let rounded: Object = call_this(&round, rounded.as_value().clone(), &[options])?
            .into_object()
            .ok_or_else(|| Exception::throw_type(&ctx, "expected Instant"))?;
        let to_zoned: Function = rounded.get("toZonedDateTimeISO")?;
        let out: Object = call_this(&to_zoned, rounded.into_value(), &["UTC".into_js(&ctx)?])?
            .into_object()
            .ok_or_else(|| Exception::throw_type(&ctx, "expected ZonedDateTime"))?;
        rehome(
            &ctx,
            construct(
                &plain_time,
                [
                    out.get("hour")?,
                    out.get("minute")?,
                    out.get("second")?,
                    out.get("millisecond")?,
                    out.get("microsecond")?,
                    out.get("nanosecond")?,
                ],
            )?,
        )
    })?;
    define_data(&brand.proto, "round", fn_)
}

fn construct_with<'js>(
    ctx: &Ctx<'js>, type_name: &str, self_value: &Value<'js>, partial: &Object<'js>,
) -> Result<Value<'js>> {
    let self_obj = js_object(self_value)?;
    let wrapped = wrapped_constructor(ctx, type_name)?;
    match type_name {
        "Duration" => rehome(
            ctx,
            construct(
                &wrapped,
                [
                    or_self(partial, &self_obj, "years")?,
                    or_self(partial, &self_obj, "months")?,
                    or_self(partial, &self_obj, "weeks")?,
                    or_self(partial, &self_obj, "days")?,
                    or_self(partial, &self_obj, "hours")?,
                    or_self(partial, &self_obj, "minutes")?,
                    or_self(partial, &self_obj, "seconds")?,
                    or_self(partial, &self_obj, "milliseconds")?,
                    or_self(partial, &self_obj, "microseconds")?,
                    or_self(partial, &self_obj, "nanoseconds")?,
                ],
            )?,
        ),
        "PlainDate" => rehome(
            ctx,
            construct(
                &wrapped,
                [
                    or_self(partial, &self_obj, "year")?,
                    month_from_partial(ctx, partial, self_obj.get("month")?)?,
                    or_self(partial, &self_obj, "day")?,
                    self_obj.get("calendarId")?,
                ],
            )?,
        ),
        "PlainTime" => rehome(
            ctx,
            construct(
                &wrapped,
                [
                    or_self(partial, &self_obj, "hour")?,
                    or_self(partial, &self_obj, "minute")?,
                    or_self(partial, &self_obj, "second")?,
                    or_self(partial, &self_obj, "millisecond")?,
                    or_self(partial, &self_obj, "microsecond")?,
                    or_self(partial, &self_obj, "nanosecond")?,
                ],
            )?,
        ),
        "PlainDateTime" => rehome(
            ctx,
            construct(
                &wrapped,
                [
                    or_self(partial, &self_obj, "year")?,
                    month_from_partial(ctx, partial, self_obj.get("month")?)?,
                    or_self(partial, &self_obj, "day")?,
                    or_self(partial, &self_obj, "hour")?,
                    or_self(partial, &self_obj, "minute")?,
                    or_self(partial, &self_obj, "second")?,
                    or_self(partial, &self_obj, "millisecond")?,
                    or_self(partial, &self_obj, "microsecond")?,
                    or_self(partial, &self_obj, "nanosecond")?,
                    self_obj.get("calendarId")?,
                ],
            )?,
        ),
        "PlainYearMonth" => rehome(
            ctx,
            construct(
                &wrapped,
                [
                    or_self(partial, &self_obj, "year")?,
                    month_from_partial(ctx, partial, self_obj.get("month")?)?,
                    self_obj.get("calendarId")?,
                ],
            )?,
        ),
        "PlainMonthDay" => {
            let fallback = parse_month_code(ctx, &self_obj.get("monthCode")?)?;
            rehome(
                ctx,
                construct(
                    &wrapped,
                    [
                        month_from_partial(ctx, partial, fallback.into_js(ctx)?)?,
                        or_self(partial, &self_obj, "day")?,
                        self_obj.get("calendarId")?,
                    ],
                )?,
            )
        }
        "ZonedDateTime" => {
            let bag = Object::new(ctx.clone())?;
            bag.set("year", or_self(partial, &self_obj, "year")?)?;
            bag.set(
                "month",
                month_from_partial(ctx, partial, self_obj.get("month")?)?,
            )?;
            bag.set("day", or_self(partial, &self_obj, "day")?)?;
            bag.set("hour", or_self(partial, &self_obj, "hour")?)?;
            bag.set("minute", or_self(partial, &self_obj, "minute")?)?;
            bag.set("second", or_self(partial, &self_obj, "second")?)?;
            bag.set("millisecond", or_self(partial, &self_obj, "millisecond")?)?;
            bag.set("microsecond", or_self(partial, &self_obj, "microsecond")?)?;
            bag.set("nanosecond", or_self(partial, &self_obj, "nanosecond")?)?;
            bag.set("timeZone", self_obj.get::<_, Value>("timeZoneId")?)?;
            bag.set("calendar", self_obj.get::<_, Value>("calendarId")?)?;
            bag.set("offset", or_self(partial, &self_obj, "offset")?)?;
            let from: Function = wrapped.get("from")?;
            rehome(ctx, from.call::<_, Value>((bag,))?)
        }
        _ => Err(Exception::throw_type(
            ctx,
            &format!("{type_name}.with is not implemented"),
        )),
    }
}

fn wrap_now<'js>(ctx: &Ctx<'js>, now: &Object<'js>) -> Result<()> {
    let names: Vec<String> = now
        .own_keys::<String>(Filter::new().string())
        .collect::<Result<_>>()?;
    for name in names {
        let value: Value = now.get(name.as_str())?;
        if !value.is_function() {
            continue;
        }
        let method = name.clone();
        let wrapped = make_fn(ctx, &name, 0, move |ctx, _, args| {
            rehome(&ctx, call_now(&ctx, &method, args)?)
        })?;
        define_data(now, &name, wrapped)?;
    }
    Ok(())
}

fn call_now<'js>(ctx: &Ctx<'js>, name: &str, args: Vec<Value<'js>>) -> Result<Value<'js>> {
    let time_zone = Opt(args.into_iter().next());
    match name {
        "instant" => crate::now::instant(ctx.clone())?.into_js(ctx),
        "timeZoneId" => crate::now::time_zone_id(ctx.clone())?.into_js(ctx),
        "plainDateISO" => crate::now::plain_date_iso(time_zone, ctx.clone())?.into_js(ctx),
        "plainTimeISO" => crate::now::plain_time_iso(time_zone, ctx.clone())?.into_js(ctx),
        "plainDateTimeISO" => crate::now::plain_date_time_iso(time_zone, ctx.clone())?.into_js(ctx),
        "zonedDateTimeISO" => crate::now::zoned_date_time_iso(time_zone, ctx.clone())?.into_js(ctx),
        _ => Err(Exception::throw_type(ctx, "Temporal.Now method missing")),
    }
}

fn make_fn<'js, F>(ctx: &Ctx<'js>, name: &str, length: usize, apply: F) -> Result<Function<'js>>
where
    F: Fn(Ctx<'js>, Value<'js>, Vec<Value<'js>>) -> Result<Value<'js>> + 'js,
{
    Function::new(
        ctx.clone(),
        move |ctx: Ctx<'js>, this: This<Value<'js>>, args: Rest<Value<'js>>| {
            apply(ctx, this.0, args.0)
        },
    )?
    .with_name(name)?
    .with_length(length)
}

fn define_named_getter<'js, F, P>(target: &Object<'js>, name: &str, getter: F) -> Result<()>
where
    F: rquickjs::function::IntoJsFunc<'js, P> + 'js,
{
    target.prop(name, Accessor::from(getter).configurable())?;
    stamp_installed_getter(target, name)
}

fn stamp_installed_getter(target: &Object<'_>, name: &str) -> Result<()> {
    let Some(desc) = get_own_descriptor(target, name)? else {
        return Ok(());
    };
    let Ok(getter) = desc.get::<_, Function>("get") else {
        return Ok(());
    };
    getter.set_name(format!("get {name}"))?;
    getter.set_length(0)
}

fn define_data<'js>(target: &Object<'js>, name: &str, value: Function<'js>) -> Result<()> {
    define_data_value(target, name, value.into_value())
}

fn hide(target: &Object<'_>, name: &str) -> Result<()> {
    let value: Value = target.get(name)?;
    define_data_value(target, name, value)
}

fn define_data_value<'js>(target: &Object<'js>, name: &str, value: Value<'js>) -> Result<()> {
    let desc = Object::new(target.ctx().clone())?;
    desc.set("value", value)?;
    desc.set("writable", true)?;
    desc.set("enumerable", false)?;
    desc.set("configurable", true)?;
    define_property(target, name, desc)
}

fn tag(target: &Object<'_>, value: &str) -> Result<()> {
    target.prop(
        PredefinedAtom::SymbolToStringTag,
        Property::from(value).configurable(),
    )
}

fn rehome<'js>(ctx: &Ctx<'js>, value: Value<'js>) -> Result<Value<'js>> {
    if value.is_null() || !value.is_object() || value.is_function() {
        return Ok(value);
    }
    let Some(object) = value.as_object() else {
        return Ok(value);
    };
    let current = object.get_prototype();
    let temporal: Object = ctx.globals().get("Temporal")?;
    for name in INTERFACES {
        let original_proto = original_prototype(ctx, name)?;
        if current.as_ref() == Some(&original_proto) {
            let wrapped: Constructor = temporal.get(name)?;
            let proto: Object = wrapped.get(PredefinedAtom::Prototype)?;
            object.set_prototype(Some(&proto))?;
            break;
        }
    }
    Ok(value)
}

fn is_brand<'js>(ctx: &Ctx<'js>, value: &Value<'js>, type_name: &str) -> Result<bool> {
    if value.is_null() || !value.is_object() || value.is_function() {
        return Ok(false);
    }
    let Some(object) = value.as_object() else {
        return Ok(false);
    };
    let wrapped = wrapped_constructor(ctx, type_name)?;
    let original_proto = original_prototype(ctx, type_name)?;
    Ok(object.is_instance_of(&wrapped) || object.get_prototype().as_ref() == Some(&original_proto))
}

const ORIGINALS_KEY: &str = "den.temporal.originals";

fn interned_symbol<'js>(ctx: &Ctx<'js>, key: &str) -> Result<Symbol<'js>> {
    let ctor: Function = ctx.globals().get(PredefinedAtom::Symbol)?;
    let for_fn: Function = ctor.get("for")?;
    for_fn.call((key,))
}

pub fn original_constructor<'js>(ctx: &Ctx<'js>, name: &str) -> Result<Constructor<'js>> {
    let temporal: Object = ctx.globals().get("Temporal")?;
    let bag: Object = temporal.get(interned_symbol(ctx, ORIGINALS_KEY)?)?;
    bag.get(name)
}

fn original_prototype<'js>(ctx: &Ctx<'js>, name: &str) -> Result<Object<'js>> {
    original_constructor(ctx, name)?.get(PredefinedAtom::Prototype)
}

fn wrapped_constructor<'js>(ctx: &Ctx<'js>, name: &str) -> Result<Constructor<'js>> {
    let temporal: Object = ctx.globals().get("Temporal")?;
    temporal.get(name)
}

fn is_js_object(value: &Value<'_>) -> bool {
    !value.is_null() && (value.is_object() || value.is_function())
}

fn js_object<'js>(value: &Value<'js>) -> Result<Object<'js>> {
    if let Some(object) = value.as_object() {
        return Ok(object.clone());
    }
    if let Some(func) = value.as_function() {
        return Ok(func.clone().into_inner());
    }
    Err(Exception::throw_type(
        value.ctx(),
        "argument must be an object",
    ))
}

fn call_this<'js>(
    func: &Function<'js>, this: Value<'js>, args: &[Value<'js>],
) -> Result<Value<'js>> {
    let mut call = Args::new(func.ctx().clone(), args.len());
    call.this(this)?;
    for arg in args {
        call.push_arg(arg.clone())?;
    }
    func.call_arg(call)
}

fn construct<'js>(
    ctor: &Constructor<'js>, args: impl IntoIterator<Item = Value<'js>>,
) -> Result<Value<'js>> {
    let mut call = Args::new_unsized(ctor.ctx().clone());
    for arg in args {
        call.push_arg(arg)?;
    }
    ctor.construct_args(call)
}

fn construct_with_new_target<'js>(
    ctor: &Constructor<'js>, new_target: &Value<'js>, args: Vec<Value<'js>>,
) -> Result<Value<'js>> {
    let mut call = Args::new(ctor.ctx().clone(), args.len());
    call.this(new_target.clone())?;
    for arg in args {
        call.push_arg(arg)?;
    }
    ctor.construct_args(call)
}

fn arg_at<'js>(ctx: &Ctx<'js>, args: &[Value<'js>], index: usize) -> Value<'js> {
    args.get(index)
        .cloned()
        .unwrap_or_else(|| Value::new_undefined(ctx.clone()))
}

fn or_self<'js>(partial: &Object<'js>, self_obj: &Object<'js>, key: &str) -> Result<Value<'js>> {
    get_defined(partial, key)?.map_or_else(|| self_obj.get(key), Ok)
}

fn month_from_partial<'js>(
    ctx: &Ctx<'js>, partial: &Object<'js>, fallback: Value<'js>,
) -> Result<Value<'js>> {
    let month = get_defined(partial, "month")?;
    let month_code = get_defined(partial, "monthCode")?;
    match (month, month_code) {
        (Some(month), Some(code)) => {
            let from_code = parse_month_code(ctx, &code)?;
            if month.as_number() != Some(from_code) {
                return Err(Exception::throw_range(
                    ctx,
                    "month and monthCode must agree",
                ));
            }
            Ok(month)
        }
        (Some(month), None) => Ok(month),
        (None, Some(code)) => parse_month_code(ctx, &code)?.into_js(ctx),
        (None, None) => Ok(fallback),
    }
}

fn parse_month_code<'js>(ctx: &Ctx<'js>, code: &Value<'js>) -> Result<f64> {
    let text = js_to_string(ctx, code)?;
    match text.as_bytes() {
        [b'M', tens, ones] if tens.is_ascii_digit() && ones.is_ascii_digit() => {
            Ok(f64::from((tens - b'0') * 10 + (ones - b'0')))
        }
        _ => Err(Exception::throw_range(ctx, "invalid monthCode")),
    }
}

fn to_temporal_overflow<'js>(ctx: &Ctx<'js>, options: Option<&Value<'js>>) -> Result<String> {
    let Some(options) = options else {
        return Ok("constrain".to_owned());
    };
    if options.is_undefined() {
        return Ok("constrain".to_owned());
    }
    if !is_js_object(options) {
        return Err(Exception::throw_type(ctx, "options must be an object"));
    }
    let object = js_object(options)?;
    let Some(value) = get_defined(&object, "overflow")? else {
        return Ok("constrain".to_owned());
    };
    let name = js_to_string(ctx, &value)?;
    if name != "constrain" && name != "reject" {
        return Err(Exception::throw_range(ctx, "invalid overflow option"));
    }
    Ok(name)
}

fn require_options_bag<'js>(ctx: &Ctx<'js>, spec_name: &str, args: &[Value<'js>]) -> Result<()> {
    if spec_name != "round" && spec_name != "total" {
        return Ok(());
    }
    let Some(options) = args.first() else {
        return Err(Exception::throw_type(ctx, "options are required"));
    };
    if options.is_undefined() {
        return Err(Exception::throw_type(ctx, "options are required"));
    }
    if options.is_null() || !(options.is_object() || options.is_function() || options.is_string()) {
        return Err(Exception::throw_type(
            ctx,
            "options must be an object or string",
        ));
    }
    Ok(())
}

fn own_function<'js>(object: &Object<'js>, key: &str) -> Result<Option<Function<'js>>> {
    let Some(desc) = get_own_descriptor(object, key)? else {
        return Ok(None);
    };
    let value: Value = desc.get("value")?;
    Ok(value.into_function())
}

fn own_getter<'js>(object: &Object<'js>, key: &str) -> Result<Option<Function<'js>>> {
    let Some(desc) = get_own_descriptor(object, key)? else {
        return Ok(None);
    };
    let get: Value = desc.get("get")?;
    Ok(get.into_function())
}

fn function_length(func: &Function<'_>) -> Result<usize> {
    let value: Value = func.get(PredefinedAtom::Length)?;
    Ok(value.as_number().unwrap_or(0.0) as usize)
}

fn uncapitalize(name: &str) -> String {
    let mut chars = name.chars();
    chars.next().map_or_else(String::new, |first| {
        first.to_lowercase().collect::<String>() + chars.as_str()
    })
}

fn object_ctor<'js>(ctx: &Ctx<'js>) -> Result<Object<'js>> {
    ctx.globals().get(PredefinedAtom::Object)
}

/// `Object.getOwnPropertyDescriptor` — rquickjs has no descriptor getter.
pub fn get_own_descriptor<'js>(object: &Object<'js>, key: &str) -> Result<Option<Object<'js>>> {
    let get: Function = object_ctor(object.ctx())?.get("getOwnPropertyDescriptor")?;
    let desc: Value = get.call((object.clone(), key))?;
    Ok(desc.into_object())
}

/// `Object.defineProperty` for copying a full descriptor.
pub fn define_property<'js>(object: &Object<'js>, key: &str, desc: Object<'js>) -> Result<()> {
    let define: Function = object_ctor(object.ctx())?.get("defineProperty")?;
    define.call((object.clone(), key, desc))
}
