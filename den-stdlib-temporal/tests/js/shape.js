import { assert, assertEquals, assertStrictEquals, assertThrows } from "den:assert";

// Statics are the plain rquickjs functions: a missing required argument is a
// TypeError from the engine, not a padded undefined.
for (const type of ["Instant", "Duration", "PlainDate", "PlainTime", "PlainDateTime", "PlainYearMonth", "PlainMonthDay", "ZonedDateTime"]) {
  assertThrows(() => Temporal[type].from(), TypeError, undefined, `${type}.from()`);
  assertStrictEquals(Temporal[type].from.length, 1);
  assertStrictEquals(Temporal[type].from.name, "from");
}
assertEquals(Temporal.PlainDate.from("2020-01-02", undefined).toString(), "2020-01-02");
assertThrows(() => Temporal.PlainDate.from("2020-01-02", null), TypeError);

// Temporal.Now forwards its time zone argument to the Rust function.
assertStrictEquals(Temporal.Now.zonedDateTimeISO("Asia/Tokyo").timeZoneId, "Asia/Tokyo");
assertStrictEquals(Temporal.Now.zonedDateTimeISO("+05:30").offset, "+05:30");
assertThrows(() => Temporal.Now.plainDateISO("Not/AZone"), RangeError);
assert(Temporal.Now.plainDateISO("UTC") instanceof Temporal.PlainDate);
assert(Temporal.Now.instant() instanceof Temporal.Instant);
assertStrictEquals(typeof Temporal.Now.timeZoneId(), "string");
assertStrictEquals(Temporal.Now.plainDateISO.name, "plainDateISO");
assertStrictEquals(Temporal.Now.plainDateISO.length, 0);

// PlainMonthDay is shaped like its siblings: constructor length 2, `from`
// static, prototype.constructor identity, and subclass NewTarget honoured.
assertStrictEquals(Temporal.PlainMonthDay.length, 2);
assertStrictEquals(Temporal.PlainMonthDay.prototype.constructor, Temporal.PlainMonthDay);
const fromDescriptor = Object.getOwnPropertyDescriptor(Temporal.PlainMonthDay, "from");
assertEquals([fromDescriptor.writable, fromDescriptor.enumerable, fromDescriptor.configurable], [true, false, true]);
class SubMonthDay extends Temporal.PlainMonthDay {}
const sub = new SubMonthDay(12, 25);
assert(sub instanceof SubMonthDay);
assert(sub instanceof Temporal.PlainMonthDay);
assertStrictEquals(sub.monthCode, "M12");
assertStrictEquals(sub.day, 25);
assert(SubMonthDay.from("12-25") instanceof Temporal.PlainMonthDay);
assertThrows(() => new Temporal.PlainMonthDay(13, 1), RangeError);
assertThrows(() => Temporal.PlainMonthDay(1, 1), TypeError);
const temporalDescriptor = Object.getOwnPropertyDescriptor(globalThis, "Temporal");
assertEquals([temporalDescriptor.writable, temporalDescriptor.enumerable, temporalDescriptor.configurable], [true, false, true]);
assertStrictEquals(typeof temporalDescriptor.value, "object");
