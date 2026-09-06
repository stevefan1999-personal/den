// cargo run -- examples/intl-temporal.ts
//
// Temporal is global (temporal_rs). Intl is phase 1: the namespace,
// getCanonicalLocales, and Locale over ICU4X. Formatters that need CLDR
// data are absent rather than stubbed.

export {};

const date = Temporal.PlainDate.from("2026-09-06");
console.log("tomorrow", date.add({ days: 1 }).toString());
console.log("now", Temporal.Now.instant().toString());
console.log("canonical", Intl.getCanonicalLocales(["EN-us", "de-de"]).join(","));

const locale = new Intl.Locale("und");
console.log("maximize", locale.maximize().toString());
console.log("arabic direction", new Intl.Locale("ar").getTextInfo().direction);
