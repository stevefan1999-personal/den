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
    name:           &'static str,
    original:       Constructor<'js>,
    wrapped:        Constructor<'js>,
    original_proto: Object<'js>,
    proto:          Object<'js>,
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

fn required_statics(type_name: &str) -> &'static [&'static str] {
    match type_name {
        "Instant" => {
            &[
                "from",
                "compare",
                "fromEpochNanoseconds",
                "fromEpochMilliseconds",
            ]
        }
        "PlainMonthDay" => &["from"],
        "Duration" | "PlainDate" | "PlainTime" | "PlainDateTime" | "PlainYearMonth"
        | "ZonedDateTime" => &["from", "compare"],
        _ => &[],
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
            call_this(&original_from, original.as_value().clone(), &[
                first, second,
            ])?,
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
