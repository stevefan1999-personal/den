//! `Intl.Locale`: a parsed, canonical Unicode locale identifier plus the
//! locale-info accessors ECMA-402 hangs off it.

use den_util::{Probe as _, coerce_string};
use icu_calendar::{
    AnyCalendar, AnyCalendarKind, Calendar as _,
    preferences::CalendarPreferences,
    week::{WeekInformation, WeekPreferences},
};
use icu_locale::{Direction, LocaleDirectionality, LocaleExpander};
use icu_locale_core::{
    LanguageIdentifier,
    extensions::unicode::{Key, Value, key},
    langid,
    subtags::{Language, Region, Script, Variant, Variants},
};
use rquickjs::{
    Ctx, Exception, IntoJs, JsLifetime, Result,
    class::{Class, Trace},
    prelude::Opt,
};

use crate::options::{Bcp47, Options};

/// The Unicode extension keys `Intl.Locale` mirrors as options and accessors,
/// in the order the constructor reads them.
const CALENDAR: Key = key!("ca");
const COLLATION: Key = key!("co");
const FIRST_DAY: Key = key!("fw");
const HOUR_CYCLE: Key = key!("hc");
const CASE_FIRST: Key = key!("kf");
const NUMERIC: Key = key!("kn");
const NUMBERING: Key = key!("nu");

/// `firstDayOfWeek` accepts ISO weekday numbers as well as CLDR day codes;
/// index 0 and 7 are both Sunday.
const WEEKDAY_CODES: [&str; 8] = ["sun", "mon", "tue", "wed", "thu", "fri", "sat", "sun"];

/// CLDR's root likely-subtags entry. ICU4X deliberately leaves a bare `und`
/// alone, where `AddLikelySubtags` is defined to expand it like any other tag.
const ROOT_EXPANSION: LanguageIdentifier = langid!("en-Latn-US");

#[derive(Clone, Trace, JsLifetime)]
#[rquickjs::class(rename = "Locale")]
pub struct Locale {
    #[qjs(skip_trace)]
    tag: icu_locale_core::Locale,
}

#[rquickjs::methods(rename_all = "camelCase")]
impl Locale {
    /// `Intl.Locale(tag [, options])`. The tag is canonicalized *before* the
    /// options are applied and again afterwards: CLDR alias rules can rewrite
    /// a tag arbitrarily, so `new Intl.Locale("und-Armn-SU", {language: "ru"})`
    /// has to resolve `SU` to `AM` before `ru` replaces the language.
    #[qjs(constructor)]
    pub fn new<'js>(
        ctx: Ctx<'js>, tag: rquickjs::Value<'js>, options: Opt<rquickjs::Value<'js>>,
    ) -> Result<Self> {
        let source = Self::tag_source(&ctx, tag)?;
        let options = Options::coerce(&ctx, options)?;
        let mut tag = Bcp47::canonical(&ctx, &source)?;
        Self::update_language_id(&ctx, &mut tag, &options)?;
        Self::update_keywords(&ctx, &mut tag, &options)?;
        Bcp47::canonicalize(&mut tag);
        Ok(Self { tag })
    }

    #[qjs(get, configurable)]
    pub fn base_name(&self) -> String { self.tag.id.to_string() }

    #[qjs(get, configurable)]
    pub fn language(&self) -> String { self.tag.id.language.to_string() }

    #[qjs(get, configurable)]
    pub fn script(&self) -> Option<String> { self.tag.id.script.map(|script| script.to_string()) }

    #[qjs(get, configurable)]
    pub fn region(&self) -> Option<String> { self.tag.id.region.map(|region| region.to_string()) }

    #[qjs(get, configurable)]
    pub fn variants(&self) -> Option<String> {
        (!self.tag.id.variants.is_empty()).then(|| self.tag.id.variants.to_string())
    }

    #[qjs(get, configurable)]
    pub fn calendar(&self) -> Option<String> { self.keyword(CALENDAR) }

    #[qjs(get, configurable)]
    pub fn collation(&self) -> Option<String> { self.keyword(COLLATION) }

    #[qjs(get, configurable)]
    pub fn first_day_of_week(&self) -> Option<String> { self.keyword(FIRST_DAY) }

    #[qjs(get, configurable)]
    pub fn hour_cycle(&self) -> Option<String> { self.keyword(HOUR_CYCLE) }

    #[qjs(get, configurable)]
    pub fn case_first(&self) -> Option<String> { self.keyword(CASE_FIRST) }

    /// `kn` with no value means "true"; the key being absent means "false".
    #[qjs(get, configurable)]
    pub fn numeric(&self) -> bool { self.keyword(NUMERIC).is_some_and(|value| value != "false") }

    #[qjs(get, configurable)]
    pub fn numbering_system(&self) -> Option<String> { self.keyword(NUMBERING) }

    #[qjs(rename = "toString")]
    pub fn to_js_string(&self) -> String { self.tag.to_string() }

    /// `AddLikelySubtags` over the language identifier; extensions ride along.
    pub fn maximize(&self) -> Self { self.transformed(Self::add_likely_subtags) }

    /// `RemoveLikelySubtags`, which is defined to add the likely subtags first
    /// so that `und` and `und-Thai` minimize to real languages.
    pub fn minimize(&self) -> Self {
        self.transformed(|id| {
            Self::add_likely_subtags(id);
            LocaleExpander::new_extended().minimize(id);
        })
    }

    /// `CalendarsOfLocale`. An explicit `ca` keyword wins; otherwise ICU4X
    /// resolves the region's preferred calendar from CLDR, honouring the `rg`
    /// region override.
    ///
    /// ponytail: ICU4X answers with the single preferred calendar rather than
    /// CLDR's whole preference list, so this array is always one element long.
    /// Widen it when den vendors `calendarPreferenceData`.
    pub fn get_calendars(&self) -> Vec<String> {
        if let Some(calendar) = self.keyword(CALENDAR) {
            return vec![calendar];
        }
        let kind = AnyCalendarKind::try_new(CalendarPreferences::from(&self.tag))
            .unwrap_or(AnyCalendarKind::Gregorian);
        AnyCalendar::new(kind)
            .calendar_algorithm()
            .map(|algorithm| Value::from(algorithm).to_string())
            .into_iter()
            .collect()
    }

    /// `CollationsOfLocale`.
    ///
    /// ponytail: ICU4X's collation data is baked and not enumerable, so only an
    /// explicit `co` keyword can be reported. Fill the rest in when den carries
    /// CLDR's per-locale collation list.
    pub fn get_collations(&self) -> Vec<String> { self.keyword(COLLATION).into_iter().collect() }

    /// `HourCyclesOfLocale`.
    ///
    /// ponytail: as for collations — the per-region hour-cycle preference is
    /// not exposed by ICU4X 2.x.
    pub fn get_hour_cycles(&self) -> Vec<String> { self.keyword(HOUR_CYCLE).into_iter().collect() }

    /// `NumberingSystemsOfLocale`.
    ///
    /// ponytail: the per-locale default numbering system lives in
    /// `icu_decimal`'s data, which den does not link.
    pub fn get_numbering_systems(&self) -> Vec<String> {
        self.keyword(NUMBERING).into_iter().collect()
    }

    /// `TimeZonesOfLocale`: `undefined` for a locale with no region, which is
    /// the whole of the answer ICU4X can support.
    ///
    /// ponytail: the region-to-zone table lives in CLDR's `metaZones`, not in
    /// any ICU4X data marker den links.
    pub fn get_time_zones(&self) -> Option<Vec<String>> { self.tag.id.region.map(|_| Vec::new()) }

    /// `GetTextInfo`: the writing direction of the locale's script.
    pub fn get_text_info(&self) -> TextInfo {
        let right_to_left =
            LocaleDirectionality::new_extended().get(&self.tag.id) == Some(Direction::RightToLeft);
        TextInfo {
            direction: if right_to_left { "rtl" } else { "ltr" },
        }
    }

    /// `GetWeekInfo`: CLDR week data, with `fw` and `rg` overrides applied by
    /// ICU4X.
    pub fn get_week_info(&self) -> WeekInfo {
        let Ok(week) = WeekInformation::try_new(WeekPreferences::from(&self.tag)) else {
            return WeekInfo::ISO;
        };
        let mut weekend: Vec<u8> = week.weekend().map(|day| day as u8).collect();
        // ICU4X iterates the weekend from the locale's first weekday; the spec
        // wants ascending ISO day numbers.
        weekend.sort_unstable();
        WeekInfo {
            first_day: week.first_weekday as u8,
            weekend,
        }
    }
}

impl Locale {
    pub const TO_STRING_TAG: &'static str = "Intl.Locale";

    /// `CanonicalizeLocaleList`'s per-element step: an `Intl.Locale` argument
    /// contributes its own canonical tag rather than going through ToString.
    pub fn tag_source<'js>(ctx: &Ctx<'js>, value: rquickjs::Value<'js>) -> Result<String> {
        if let Some(object) = value.as_object() {
            if let Some(locale) = ctx.probe(|| Class::<Self>::from_object(object)) {
                return Ok(locale.borrow().tag.to_string());
            }
        } else if !value.is_string() {
            return Err(Exception::throw_type(
                ctx,
                "a language tag must be a string or an object",
            ));
        }
        coerce_string(ctx, value)
    }

    fn add_likely_subtags(id: &mut LanguageIdentifier) {
        if id.language.is_unknown() && id.script.is_none() && id.region.is_none() {
            id.language = ROOT_EXPANSION.language;
            id.script = ROOT_EXPANSION.script;
            id.region = ROOT_EXPANSION.region;
            return;
        }
        LocaleExpander::new_extended().maximize(id);
    }

    fn keyword(&self, key: Key) -> Option<String> {
        self.tag
            .extensions
            .unicode
            .keywords
            .get(&key)
            .map(ToString::to_string)
    }

    fn transformed(
        &self, transform: impl FnOnce(&mut icu_locale_core::LanguageIdentifier),
    ) -> Self {
        let mut tag = self.tag.clone();
        transform(&mut tag.id);
        Bcp47::canonicalize(&mut tag);
        Self { tag }
    }

    /// `UpdateLanguageId`: every component the option bag names replaces the
    /// one parsed out of the tag.
    fn update_language_id(
        ctx: &Ctx<'_>, tag: &mut icu_locale_core::Locale, options: &Options<'_>,
    ) -> Result<()> {
        if let Some(language) = options.string("language", &[])? {
            tag.id.language = Self::subtag(ctx, "language", &language, Language::try_from_str)?;
        }
        if let Some(script) = options.string("script", &[])? {
            tag.id.script = Some(Self::subtag(ctx, "script", &script, Script::try_from_str)?);
        }
        if let Some(region) = options.string("region", &[])? {
            tag.id.region = Some(Self::subtag(ctx, "region", &region, Region::try_from_str)?);
        }
        if let Some(variants) = options.string("variants", &[])? {
            tag.id.variants = Self::variant_list(ctx, &variants)?;
        }
        Ok(())
    }

    /// The Unicode extension keys `Intl.Locale` exposes as options.
    fn update_keywords(
        ctx: &Ctx<'_>, tag: &mut icu_locale_core::Locale, options: &Options<'_>,
    ) -> Result<()> {
        const HOUR_CYCLES: [&str; 4] = ["h11", "h12", "h23", "h24"];
        const CASE_ORDERS: [&str; 3] = ["upper", "lower", "false"];

        let mut set = |key: Key, value: Option<String>| -> Result<()> {
            let Some(value) = value else { return Ok(()) };
            let value = Self::keyword_value(ctx, &value)?;
            tag.extensions.unicode.keywords.set(key, value);
            Ok(())
        };
        set(CALENDAR, options.string("calendar", &[])?)?;
        set(COLLATION, options.string("collation", &[])?)?;
        set(FIRST_DAY, Self::first_day(options)?)?;
        set(HOUR_CYCLE, options.string("hourCycle", &HOUR_CYCLES)?)?;
        set(CASE_FIRST, options.string("caseFirst", &CASE_ORDERS)?)?;
        set(
            NUMERIC,
            options
                .boolean("numeric")?
                .map(|numeric| if numeric { "true" } else { "false" }.to_string()),
        )?;
        set(NUMBERING, options.string("numberingSystem", &[])?)?;
        Ok(())
    }

    /// `WeekdayToString`: an ISO weekday number names the same day as its CLDR
    /// code, and anything else is passed through as a plain keyword value.
    fn first_day(options: &Options<'_>) -> Result<Option<String>> {
        Ok(options.string("firstDayOfWeek", &[])?.map(|value| {
            value
                .parse::<usize>()
                .ok()
                .and_then(|day| WEEKDAY_CODES.get(day))
                .map_or_else(|| value.clone(), |code| (*code).to_string())
        }))
    }

    fn subtag<T, E>(
        ctx: &Ctx<'_>, what: &str, source: &str,
        parse: impl FnOnce(&str) -> core::result::Result<T, E>,
    ) -> Result<T> {
        parse(source).map_err(|_unparseable| {
            Exception::throw_range(ctx, &format!("{source:?} is not a valid {what} subtag"))
        })
    }

    /// A `unicode_variant_subtag` sequence, rejected on a repeat.
    fn variant_list(ctx: &Ctx<'_>, source: &str) -> Result<Variants> {
        let mut variants = Variants::new();
        for part in source.split('-') {
            let variant = Self::subtag(ctx, "variant", part, Variant::try_from_str)?;
            if !variants.push(variant) {
                return Err(Exception::throw_range(
                    ctx,
                    &format!("{part:?} appears twice in the variants option"),
                ));
            }
        }
        Ok(variants)
    }

    /// A `unicode_type` keyword value: `(3*8alphanum) *("-" 3*8alphanum)`.
    /// ICU4X's `Subtag` accepts one to eight characters, so the narrower length
    /// the extension grammar demands is checked here.
    fn keyword_value(ctx: &Ctx<'_>, source: &str) -> Result<Value> {
        const SUBTAG_LENGTH: core::ops::RangeInclusive<usize> = 3..=8;
        if !source.split('-').all(|part| {
            SUBTAG_LENGTH.contains(&part.len())
                && part.bytes().all(|byte| byte.is_ascii_alphanumeric())
        }) {
            return Err(Exception::throw_range(
                ctx,
                &format!("{source:?} is not a valid option value"),
            ));
        }
        Self::subtag(ctx, "keyword value", source, Value::try_from_str)
    }
}

/// The `getTextInfo` result. Its own type rather than an `Object` so that the
/// class impl block stays free of the `'js` lifetime.
#[derive(IntoJs)]
pub struct TextInfo {
    direction: &'static str,
}

/// The `getWeekInfo` result: ISO weekday numbers, Monday being 1.
#[derive(IntoJs)]
#[qjs(rename_all = "camelCase")]
pub struct WeekInfo {
    first_day: u8,
    weekend:   Vec<u8>,
}

impl WeekInfo {
    const ISO: Self = Self {
        first_day: 1,
        weekend:   Vec::new(),
    };
}
