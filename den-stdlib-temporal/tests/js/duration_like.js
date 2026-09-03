import { assertEquals, assertThrows } from "den:assert";

// One ToTemporalDuration for every caller: a Duration, an ISO string, or a
// property bag with at least one field; everything else is a TypeError.
const instant = Temporal.Instant.from("1970-01-01T00:00:00Z");
assertEquals(instant.add({ seconds: 1 }).toString(), "1970-01-01T00:00:01Z");
assertEquals(instant.add("PT1S").toString(), "1970-01-01T00:00:01Z");
assertEquals(instant.subtract(Temporal.Duration.from({ seconds: 1 })).toString(), "1969-12-31T23:59:59Z");
assertThrows(() => instant.add({}), TypeError);
assertThrows(() => instant.add({ second: 1 }), TypeError);
assertThrows(() => instant.add(5), TypeError);
assertThrows(() => instant.add(null), TypeError);
assertThrows(() => instant.subtract(undefined), TypeError);
assertThrows(() => instant.add({ seconds: 1.5 }), RangeError);
assertThrows(() => instant.add({ years: 1 }), RangeError);

assertEquals(Temporal.Duration.from({ hours: 1 }).toString(), "PT1H");
assertThrows(() => Temporal.Duration.from({}), TypeError);
assertThrows(() => Temporal.Duration.from(5), TypeError);
assertThrows(() => Temporal.Duration.compare({}, "PT0S"), TypeError);
assertThrows(() => Temporal.Duration.compare("PT0S", {}), TypeError);
assertEquals(Temporal.Duration.compare({ hours: 1 }, "PT60M"), 0);
assertEquals(Temporal.Duration.from("PT1H").add({ minutes: 30 }).toString(), "PT1H30M");
assertThrows(() => Temporal.Duration.from("PT1H").subtract({}), TypeError);
