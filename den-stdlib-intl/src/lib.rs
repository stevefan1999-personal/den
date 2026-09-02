//! ECMA-402 `Intl` for den, phase 1: everything that needs no formatter data.
//!
//! quickjs-ng ships no `Intl` at all, so the namespace, `getCanonicalLocales`
//! and `Intl.Locale` are written natively against ICU4X. The parts that would
//! need CLDR formatting data — `Collator`, `DateTimeFormat`, `NumberFormat`,
//! `PluralRules` and friends — are deliberately absent rather than stubbed.

pub mod locale;
pub mod options;

use den_util::ConstructorInstaller as _;
use rquickjs::{
    Coerced, Ctx, Exception, Filter, Function, Object, Result, Value,
    atom::PredefinedAtom,
    object::Property,
    prelude::{Opt, Rest},
};

pub use crate::{js_intl_module as js_intl, locale::Locale, options::Bcp47};

/// The `Intl` namespace object.
pub struct Intl;

impl Intl {
    /// `Intl.getCanonicalLocales` takes one argument, and `Intl.Locale` one
    /// required tag; clause 17 puts both counts in a `length` property.
    const CANONICAL_LOCALES_ARITY: usize = 1;
    const LOCALE_ARITY: usize = 1;

    /// Build the namespace and install it as the `Intl` global.
    pub fn install<'js>(ctx: &Ctx<'js>) -> Result<Object<'js>> {
        let intl = Object::new(ctx.clone())?;
        intl.prop(
            PredefinedAtom::SymbolToStringTag,
            Property::from("Intl").configurable(),
        )?;

        let canonical = Function::new(ctx.clone(), Self::get_canonical_locales)?
            .with_name("getCanonicalLocales")?
            .with_length(Self::CANONICAL_LOCALES_ARITY)?;
        intl.prop(
            "getCanonicalLocales",
            Property::from(canonical).writable().configurable(),
        )?;

        intl.install_constructor::<Locale>(Self::LOCALE_ARITY)?;
        // `install_constructor` uses a plain assignment, which leaves the
        // property enumerable; no `Intl` member is. Redefining only sets the
        // attributes it names, so the enumerable one has to go first.
        let constructor = intl.get::<_, Value<'js>>("Locale")?;
        intl.remove("Locale")?;
        intl.prop(
            "Locale",
            Property::from(constructor).writable().configurable(),
        )?;
        Self::finish_prototype(ctx)?;

        ctx.globals().prop(
            "Intl",
            Property::from(intl.clone()).writable().configurable(),
        )?;
        Ok(intl)
    }

    /// `CanonicalizeLocaleList`, exposed as `Intl.getCanonicalLocales`.
    fn get_canonical_locales<'js>(
        ctx: Ctx<'js>, locales: Opt<Value<'js>>, _rest: Rest<Value<'js>>,
    ) -> Result<Vec<String>> {
        let Some(locales) = locales.0.filter(|locales| !locales.is_undefined()) else {
            return Ok(Vec::new());
        };
        // A lone string or `Intl.Locale` stands for a one-element list, and is
        // never read as an array-like.
        let tags = if locales.is_string() || Locale::is_instance(&ctx, &locales) {
            vec![Locale::tag_source(&ctx, locales)?]
        } else {
            Self::array_like_tags(&ctx, locales)?
        };

        let mut canonical: Vec<String> = Vec::with_capacity(tags.len());
        for tag in tags {
            let tag = Bcp47::canonical(&ctx, &tag)?.to_string();
            if !canonical.contains(&tag) {
                canonical.push(tag);
            }
        }
        Ok(canonical)
    }

    /// The array-like walk: `HasProperty` before `Get`, so holes are skipped
    /// without their indices being observed twice.
    fn array_like_tags<'js>(ctx: &Ctx<'js>, locales: Value<'js>) -> Result<Vec<String>> {
        if locales.is_null() {
            return Err(Exception::throw_type(ctx, "locales cannot be null"));
        }
        let list: Object<'js> = den_util::construct(ctx, "Object", (locales,))?;
        // `LengthOfArrayLike` clamps to 2^53-1, but no array-like can carry a
        // real element past the 2^32-1 index limit.
        let length = list.get::<_, Option<Coerced<f64>>>("length")?;
        let length = length.map_or(0.0, |length| length.0);
        let length = if length.is_nan() {
            0
        } else {
            length.clamp(0.0, f64::from(u32::MAX)) as u32
        };

        let mut tags = Vec::new();
        for index in 0..length {
            if !list.contains_key(index)? {
                continue;
            }
            tags.push(Locale::tag_source(ctx, list.get::<_, Value<'js>>(index)?)?);
        }
        Ok(tags)
    }

    /// Clause 17 names every built-in accessor `get <property>` and every
    /// method by its own property name; rquickjs mints both anonymous, so the
    /// prototype is stamped once after the class is registered.
    fn finish_prototype(ctx: &Ctx<'_>) -> Result<()> {
        let Some(prototype) = rquickjs::class::Class::<Locale>::prototype(ctx)? else {
            return Ok(());
        };
        prototype.prop(
            PredefinedAtom::SymbolToStringTag,
            Property::from(Locale::TO_STRING_TAG).configurable(),
        )?;

        let describe: Function<'_> = ctx
            .globals()
            .get::<_, Object<'_>>("Object")?
            .get("getOwnPropertyDescriptor")?;
        for key in prototype.own_keys::<String>(Filter::new().string()) {
            let key = key?;
            if key == "constructor" {
                continue;
            }
            let descriptor: Object<'_> = describe.call((prototype.clone(), key.as_str()))?;
            if let Some(getter) = descriptor.get::<_, Option<Function<'_>>>("get")? {
                getter.set_name(format!("get {key}"))?;
            } else if let Some(method) = descriptor.get::<_, Option<Function<'_>>>("value")? {
                method.set_name(&key)?;
            }
        }
        Ok(())
    }
}

impl Locale {
    /// Whether `value` carries the `[[InitializedLocale]]` slot.
    fn is_instance(ctx: &Ctx<'_>, value: &Value<'_>) -> bool {
        use den_util::Probe as _;
        value.as_object().is_some_and(|object| {
            ctx.probe(|| rquickjs::class::Class::<Self>::from_object(object))
                .is_some()
        })
    }
}

#[rquickjs::module]
pub mod intl_module {
    use rquickjs::{
        Ctx, Result,
        module::{Declarations, Exports},
    };

    use super::Intl;

    #[qjs(declare)]
    pub fn declare(declarations: &Declarations) -> Result<()> {
        declarations.declare("Intl")?;
        Ok(())
    }

    #[qjs(evaluate)]
    pub fn evaluate<'js>(ctx: &Ctx<'js>, exports: &Exports<'js>) -> Result<()> {
        // The module and the global name the same object, so `instanceof` and
        // `Intl.Locale` agree across the two.
        exports.export("Intl", Intl::install(ctx)?)?;
        Ok(())
    }
}
