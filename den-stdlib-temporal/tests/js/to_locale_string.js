import { assertEquals, assertStrictEquals } from "den:assert";

// den has no Intl: toLocaleString is toString() with every argument ignored.
// Arguments must not leak through as toString options (a string first
// argument would otherwise be a TypeError "options must be an object").
const samples = [
  Temporal.Instant.from("2020-01-02T03:04:05.006Z"),
  Temporal.Duration.from({ hours: 1, minutes: 30 }),
  new Temporal.PlainDate(2020, 1, 2),
  new Temporal.PlainTime(3, 4, 5, 6),
  new Temporal.PlainDateTime(2020, 1, 2, 3, 4, 5, 6),
  new Temporal.PlainYearMonth(2020, 1),
  new Temporal.PlainMonthDay(1, 2),
  Temporal.ZonedDateTime.from("2020-01-02T03:04:05.006+09:00[Asia/Tokyo]"),
];

for (const sample of samples) {
  const proto = Object.getPrototypeOf(sample);
  const descriptor = Object.getOwnPropertyDescriptor(proto, "toLocaleString");
  assertEquals(typeof descriptor.value, "function", `${proto[Symbol.toStringTag]} has toLocaleString`);
  assertEquals(descriptor.writable, true);
  assertEquals(descriptor.enumerable, false);
  assertEquals(descriptor.configurable, true);
  assertStrictEquals(descriptor.value.name, "toLocaleString");
  assertStrictEquals(descriptor.value.length, 0);
  assertStrictEquals(
    sample.toLocaleString("en-US", { smallestUnit: "minute", calendarName: "always" }),
    sample.toString(),
    `${proto[Symbol.toStringTag]} ignores locales and options`,
  );
}
