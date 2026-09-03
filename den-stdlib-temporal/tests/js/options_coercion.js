import { assertEquals, assertStrictEquals, assertThrows } from "den:assert";

// GetOption string path is spec ToString: Symbol.toPrimitive, then toString
// before valueOf, Symbol is a TypeError, anything else stringifies and then
// fails the enum RangeError.
const start = Temporal.PlainDate.from("2020-01-01");
const end = Temporal.PlainDate.from("2020-03-01");
const prefersToString = { toString: () => "months", valueOf: () => "years" };
assertEquals(start.until(end, { largestUnit: prefersToString }).toString(), "P2M");
assertEquals(start.until(end, { largestUnit: { [Symbol.toPrimitive]: () => "months" } }).toString(), "P2M");
assertThrows(() => start.until(end, { largestUnit: Symbol("months") }), TypeError);
assertThrows(() => start.until(end, { largestUnit: null }), RangeError);
assertThrows(() => start.until(end, { largestUnit: 1 }), RangeError);
assertThrows(() => start.until(end, { roundingMode: { toString: () => "nope" } }), RangeError);
assertThrows(() => Temporal.PlainDate.from("2020-01-31").add({ months: 1 }, { overflow: Symbol() }), TypeError);
assertEquals(new Temporal.PlainTime(1, 2, 3).round({ smallestUnit: { toString: () => "minute" } }).toString(), "01:02:00");
assertEquals(new Temporal.PlainTime(1, 2, 3).toString({ fractionalSecondDigits: { toString: () => "auto" } }), "01:02:03");
assertThrows(() => new Temporal.PlainTime(1, 2, 3).toString({ fractionalSecondDigits: "nope" }), RangeError);
assertThrows(() => Temporal.Instant.from("1970-01-01T00:00Z").round({ smallestUnit: Symbol() }), TypeError);
assertThrows(() => Temporal.Duration.from("PT1H").round({ smallestUnit: null }), RangeError);
assertThrows(() => Temporal.Duration.from("PT1H").total({ unit: false }), RangeError);
assertEquals(
  Temporal.ZonedDateTime.from("2020-01-02T03:04:05+00:00[UTC]").toString({ offset: { toString: () => "never" }, timeZoneName: { toString: () => "never" } }),
  "2020-01-02T03:04:05",
);

// ToTemporalMonthCode: ToPrimitive(hint string), then the primitive must be a
// String — a number, or a toString returning one, is a TypeError, not a
// RangeError from parsing "5". Every property-bag reader shares the rule.
const bags = [
  (monthCode) => Temporal.PlainDate.from({ year: 2020, monthCode, day: 1 }),
  (monthCode) => Temporal.PlainDateTime.from({ year: 2020, monthCode, day: 1 }),
  (monthCode) => Temporal.PlainYearMonth.from({ year: 2020, monthCode }),
  (monthCode) => Temporal.PlainMonthDay.from({ monthCode, day: 1 }),
  (monthCode) => Temporal.ZonedDateTime.from({ year: 2020, monthCode, day: 1, timeZone: "UTC" }),
  (monthCode) => Temporal.Duration.compare("P1D", "PT24H", { relativeTo: { year: 2020, monthCode, day: 1 } }),
];
for (const build of bags) {
  for (const wrongType of [5, 5n, false, Symbol(), null, { toString: () => 5 }]) {
    assertThrows(() => build(wrongType), TypeError);
  }
  assertThrows(() => build("M1"), RangeError);
  assertThrows(() => build({ toString: () => "nope" }), RangeError);
  build({ [Symbol.toPrimitive]: () => "M02" });
  build({ toString: () => "M02", valueOf: () => "M03" });
}
assertStrictEquals(
  String(Temporal.PlainDate.from({ year: 2020, monthCode: { toString: () => "M02", valueOf: () => "M03" }, day: 1 })),
  "2020-02-01",
);
assertStrictEquals(String(Temporal.PlainMonthDay.from({ monthCode: { [Symbol.toPrimitive]: () => "M02" }, day: 1 })), "02-01");
