import { assertEquals, assertThrows } from "den:assert";

// ToTemporalTime on a property bag: at least one unit, ToIntegerWithTruncation
// per field, then RegulateTime with constrain — 300 hours clamps to 23 instead
// of failing an integer range check.
const date = new Temporal.PlainDate(2020, 1, 2);
const dateTime = new Temporal.PlainDateTime(2020, 1, 2, 3, 4, 5);
const zoned = Temporal.ZonedDateTime.from("2020-01-02T03:04:05+00:00[UTC]");

assertThrows(() => date.toPlainDateTime({}), TypeError);
assertThrows(() => date.toZonedDateTime({ timeZone: "UTC", plainTime: {} }), TypeError);
assertThrows(() => dateTime.withPlainTime({}), TypeError);
assertThrows(() => zoned.withPlainTime({}), TypeError);
assertThrows(() => dateTime.withPlainTime({ hour: Infinity }), RangeError);
assertThrows(() => dateTime.withPlainTime(7), TypeError);

assertEquals(date.toPlainDateTime({ hour: 300, minute: 5.9 }).toString(), "2020-01-02T23:05:00");
assertEquals(date.toZonedDateTime({ timeZone: "UTC", plainTime: { hour: 300 } }).toPlainTime().toString(), "23:00:00");
assertEquals(dateTime.withPlainTime({ minute: 30 }).toString(), "2020-01-02T00:30:00");
assertEquals(dateTime.withPlainTime({ hour: 300, second: 61 }).toString(), "2020-01-02T23:00:59");
assertEquals(zoned.withPlainTime({ hour: 300 }).toPlainTime().toString(), "23:00:00");
assertEquals(zoned.withPlainTime("12:34").toPlainTime().toString(), "12:34:00");
assertEquals(dateTime.withPlainTime(new Temporal.PlainTime(6, 7)).toString(), "2020-01-02T06:07:00");
