//! The ECMA-402 abstract operations every `Intl` constructor shares.
//!
//! These live apart from `Intl.Locale` on purpose: `Collator`,
//! `DateTimeFormat`, `NumberFormat` and the rest are each defined in terms of
//! the same four steps — coerce the option bag, read options with
//! `GetOption`/`GetBooleanOption`/`GetNumberOption`, canonicalize the requested
//! locales, then resolve them against what the constructor actually supports.

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

    /// `GetNumberOption(options, property, minimum, maximum, fallback)`:
    /// out-of-range and non-numeric values are a RangeError, not a clamp.
    pub fn number(&self, property: &str, minimum: f64, maximum: f64, fallback: f64) -> Result<f64> {
        let ctx = self.bag.ctx();
        let Some(value) = self.raw(property)? else {
            return Ok(fallback);
        };
        let value = Coerced::<f64>::from_js(ctx, value)?.0;
        if value.is_nan() || value < minimum || value > maximum {
            return Err(Exception::throw_range(
                ctx,
                &format!("{value} is out of range for {property}"),
            ));
        }
        Ok(value.floor())
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

    /// `IsStructurallyValidLanguageTag` — ICU4X's parser is the UTS-35 grammar
    /// ECMA-402 defers to, so a parse failure is exactly a RangeError.
    pub fn parse(ctx: &Ctx<'_>, tag: &str) -> Result<Locale> {
        Locale::try_from_str(tag).map_err(|error| {
            Exception::throw_range(
                ctx,
                &format!("{tag:?} is not a valid language tag: {error}"),
            )
        })
    }

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
    pub fn canonical(ctx: &Ctx<'_>, tag: &str) -> Result<Locale> {
        let mut locale = Self::parse(ctx, tag)?;
        Self::canonicalize(&mut locale);
        Ok(locale)
    }
}

/// The locales a single `Intl` constructor claims to support.
///
/// ICU4X's compiled data is baked into statics and is not enumerable, so no
/// ICU4X call can answer "which locales do you carry?". Every den `Intl`
/// constructor therefore declares its own list and answers `supportedLocalesOf`
/// from it.
pub struct AvailableLocales<'a>(pub &'a [&'a str]);

impl AvailableLocales<'_> {
    /// `BestAvailableLocale`: drop one trailing subtag at a time until what is
    /// left is on the list. Matching runs over the language identifier alone,
    /// which is `LookupMatcher`'s "remove the Unicode extension sequence" step
    /// for free.
    pub fn best(&self, candidate: &Locale) -> Option<String> {
        let mut prefix = candidate.id.to_string();
        loop {
            if self.0.contains(&prefix.as_str()) {
                return Some(prefix);
            }
            prefix.truncate(prefix.rfind('-')?);
        }
    }

    /// `ResolveLocale`'s lookup half: the first requested locale with a best
    /// available match, or the caller's default.
    pub fn resolve(&self, requested: &[Locale], default: &str) -> String {
        requested
            .iter()
            .find_map(|candidate| self.best(candidate))
            .unwrap_or_else(|| default.to_string())
    }
}

#[cfg(test)]
mod tests {
    use icu_locale_core::{
        Locale,
        extensions::unicode::{Value as KeywordValue, key},
        preferences::extensions::unicode::keywords::CalendarAlgorithm,
    };

    use super::AvailableLocales;

    fn locale(tag: &str) -> Locale { tag.parse().expect("test tag parses") }

    #[test]
    fn best_available_locale_truncates_subtag_by_subtag() {
        let available = AvailableLocales(&["en", "zh-Hant"]);
        assert_eq!(available.best(&locale("en-US")).as_deref(), Some("en"));
        assert_eq!(
            available.best(&locale("zh-Hant-TW")).as_deref(),
            Some("zh-Hant")
        );
        // Extensions never take part in matching.
        assert_eq!(
            available.best(&locale("en-US-u-ca-gregory")).as_deref(),
            Some("en")
        );
        assert_eq!(available.best(&locale("de-DE")), None);
    }

    #[test]
    fn resolve_falls_back_to_the_declared_default() {
        let available = AvailableLocales(&["en"]);
        assert_eq!(
            available.resolve(&[locale("de"), locale("en-GB")], "und"),
            "en"
        );
        assert_eq!(available.resolve(&[locale("de")], "und"), "und");
        assert_eq!(available.resolve(&[], "und"), "und");
    }
}
