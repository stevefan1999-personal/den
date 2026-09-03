//! The ECMA-402 abstract operations every `Intl` constructor shares.
//!
//! These live apart from `Intl.Locale` on purpose: `Collator`,
//! `DateTimeFormat`, `NumberFormat` and the rest are each defined in terms of
//! the same steps — coerce the option bag, read options with `GetOption` and
//! `GetBooleanOption`, then canonicalize the requested locales.

use den_util::coerce_string;
use icu_locale::LocaleCanonicalizer;
use icu_locale_core::{
    Locale,
    extensions::unicode::{Value as KeywordValue, key},
    preferences::extensions::unicode::keywords::CalendarAlgorithm,
};
use rquickjs::{Coerced, Ctx, Exception, FromJs as _, Object, Result, Value, prelude::Opt};

/// An `Intl` option bag, read the way ECMA-402 reads one.
pub struct Options<'js> {
    bag: Object<'js>,
}

impl<'js> Options<'js> {
    /// `CoerceOptionsToObject`. An absent bag becomes an object with *no*
    /// prototype, so a poisoned `Object.prototype` cannot smuggle options into
    /// `new Intl.Locale("en")`.
    pub fn coerce(ctx: &Ctx<'js>, value: Opt<Value<'js>>) -> Result<Self> {
        let Some(value) = value.0.filter(|value| !value.is_undefined()) else {
            let bag = Object::new(ctx.clone())?;
            bag.set_prototype(None)?;
            return Ok(Self { bag });
        };
        if value.is_null() {
            return Err(Exception::throw_type(ctx, "options must be an object"));
        }
        // `Object(value)` is ToObject, primitive wrappers included.
        Ok(Self {
            bag: den_util::construct(ctx, "Object", (value,))?,
        })
    }

    /// `GetOption(options, property, string, values, undefined)`. An empty
    /// `values` accepts anything the option's own grammar accepts later.
    pub fn string(&self, property: &str, values: &[&str]) -> Result<Option<String>> {
        let ctx = self.bag.ctx();
        let Some(value) = self.raw(property)? else {
            return Ok(None);
        };
        let value = coerce_string(ctx, value)?;
        if !values.is_empty() && !values.contains(&value.as_str()) {
            return Err(Exception::throw_range(
                ctx,
                &format!("{value:?} is not a supported value for {property}"),
            ));
        }
        Ok(Some(value))
    }

    /// `GetBooleanOption(options, property, undefined)`.
    pub fn boolean(&self, property: &str) -> Result<Option<bool>> {
        let ctx = self.bag.ctx();
        self.raw(property)?
            .map(|value| Coerced::<bool>::from_js(ctx, value).map(|value| value.0))
            .transpose()
    }

    /// `Get(options, property)`, with `undefined` reported as absence.
    fn raw(&self, property: &str) -> Result<Option<Value<'js>>> {
        let value = self.bag.get::<_, Value<'js>>(property)?;
        Ok((!value.is_undefined()).then_some(value))
    }
}

/// BCP-47 language tags, the ECMA-402 way.
pub struct Bcp47;

impl Bcp47 {
    /// The *extended* canonicalizer rather than the common one: complex alias
    /// rules such as `und-Armn-SU` -> `und-Armn-AM` resolve a replacement
    /// region through likely subtags, which the common data set omits.
    const CANONICALIZER: LocaleCanonicalizer = LocaleCanonicalizer::new_extended();

    /// `CanonicalizeUnicodeLocaleId`, in place.
    ///
    /// ICU4X's canonicalizer rewrites the language identifier but leaves the
    /// CLDR bcp47 *type* aliases alone; those live in the typed keyword enums,
    /// so a `ca` value is round-tripped through one to turn `islamicc` into
    /// `islamic-civil`.
    ///
    /// ponytail: `ca` is the only key whose aliases ICU4X models. `ks`, `ms`,
    /// `tz` and the `yes`/`true` spelling still canonicalize to themselves;
    /// they need CLDR's own alias table, not a widening of this call.
    pub fn canonicalize(locale: &mut Locale) {
        Self::CANONICALIZER.canonicalize(locale);
        let keywords = &mut locale.extensions.unicode.keywords;
        let calendar = keywords
            .get(&key!("ca"))
            .and_then(|value| CalendarAlgorithm::try_from(value).ok())
            .map(KeywordValue::from);
        if let Some(calendar) = calendar {
            keywords.set(key!("ca"), calendar);
        }
    }

    /// `CanonicalizeUnicodeLocaleId(IsStructurallyValidLanguageTag(tag))`.
    ///
    /// ICU4X's parser is the UTS-35 grammar ECMA-402 defers to, so a parse
    /// failure is exactly a RangeError.
    pub fn canonical(ctx: &Ctx<'_>, tag: &str) -> Result<Locale> {
        let mut locale = Locale::try_from_str(tag).map_err(|error| {
            Exception::throw_range(
                ctx,
                &format!("{tag:?} is not a valid language tag: {error}"),
            )
        })?;
        Self::canonicalize(&mut locale);
        Ok(locale)
    }
}
