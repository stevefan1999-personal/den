import { assertEquals, assert, assertThrows } from "den:assert";

assert(Temporal.Now.instant() instanceof Temporal.Instant);
assertEquals(Temporal.Instant.from("1970-01-01T00:00:00Z").epochNanoseconds, 0n);
assertEquals(Temporal.PlainDate.from("2025-03-03").year, 2025);
assertEquals(new Temporal.Duration(1, 2, 3, 4).toString(), "P1Y2M3W4D");
assertEquals(Temporal.Duration.from({ days: 5, hours: 2 }).days, 5);
assertThrows(() => new Temporal.Instant(0n).valueOf(), TypeError);
assertThrows(() => Temporal.Instant(0n), TypeError);
