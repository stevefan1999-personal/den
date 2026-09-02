import { assertEquals, assertStrictEquals, assertThrows } from "den:assert";

assertEquals(Intl.getCanonicalLocales(), []);
assertEquals(Intl.getCanonicalLocales("EN-us"), ["en-US"]);
assertEquals(Intl.getCanonicalLocales(new Intl.Locale("de-de")), ["de-DE"]);
// Duplicates collapse, order is preserved and holes are skipped.
assertEquals(Intl.getCanonicalLocales(["fr", "en-US", "FR"]), ["fr", "en-US"]);
assertEquals(Intl.getCanonicalLocales({ length: 2, 1: "es" }), ["es"]);
assertThrows(() => Intl.getCanonicalLocales("en-"), RangeError);
assertThrows(() => Intl.getCanonicalLocales([1]), TypeError);
