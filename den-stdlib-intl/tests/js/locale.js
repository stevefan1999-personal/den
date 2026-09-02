import { assertEquals, assertStrictEquals, assertThrows } from "den:assert";

// Canonicalization runs before the options are applied and again afterwards.
assertStrictEquals(new Intl.Locale("und-Armn-SU", { language: "ru" }).toString(), "ru-Armn-AM");
assertStrictEquals(new Intl.Locale("EN-latn-us-U-CA-GREGORY").toString(), "en-Latn-US-u-ca-gregory");
assertStrictEquals(new Intl.Locale("und").maximize().toString(), "en-Latn-US");
assertStrictEquals(new Intl.Locale("und").minimize().toString(), "en");

// ICU4X-backed locale info.
assertStrictEquals(new Intl.Locale("ar").getTextInfo().direction, "rtl");
assertStrictEquals(new Intl.Locale("en").getTextInfo().direction, "ltr");
assertEquals(new Intl.Locale("en-u-fw-wed").getWeekInfo().firstDay, 3);
assertEquals(new Intl.Locale("th").getCalendars(), ["buddhist"]);

// The CLDR `ca` type aliases ICU4X models, applied on canonicalization.
assertStrictEquals(new Intl.Locale("en", { calendar: "islamicc" }).calendar, "islamic-civil");
assertStrictEquals(new Intl.Locale("en-u-ca-ethiopic-amete-alem").calendar, "ethioaa");

// An option value is a `unicode_type`, so three to eight alphanumerics.
assertThrows(() => new Intl.Locale("en", { calendar: "ab" }), RangeError);
assertThrows(() => new Intl.Locale("en", { calendar: "abcdefghi" }), RangeError);
assertStrictEquals(new Intl.Locale("en", { calendar: "abc" }).calendar, "abc");
