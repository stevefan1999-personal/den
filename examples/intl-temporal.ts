// cargo run -- examples/intl-temporal.ts
//
// Temporal is global (temporal_rs). Intl is phase 1: the namespace,
// getCanonicalLocales, and Locale over ICU4X. Formatters that need CLDR
// data are absent rather than stubbed.

import { assertEquals } from "den:assert";

const date = Temporal.PlainDate.from("2026-09-06");
assertEquals(date.add({ days: 1 }).toString(), "2026-09-07");
assertEquals(Intl.getCanonicalLocales(["EN-us", "de-de"]), ["en-US", "de-DE"]);
assertEquals(new Intl.Locale("und").maximize().toString(), "en-Latn-US");
assertEquals(new Intl.Locale("ar").getTextInfo().direction, "rtl");
console.log("tomorrow", date.add({ days: 1 }).toString());
console.log("now", Temporal.Now.instant().toString());
console.log("canonical", Intl.getCanonicalLocales(["EN-us", "de-de"]).join(","));
console.log("maximize", new Intl.Locale("und").maximize().toString());
console.log("arabic direction", new Intl.Locale("ar").getTextInfo().direction);
