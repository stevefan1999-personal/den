import { assert, assertEquals } from "den:assert";

assertEquals(typeof Temporal.Now.timeZoneId(), "string");
assert(Temporal.Now.plainDateISO() instanceof Temporal.PlainDate);
assert(Temporal.Now.plainTimeISO() instanceof Temporal.PlainTime);
assert(Temporal.Now.plainDateTimeISO() instanceof Temporal.PlainDateTime);
assert(Temporal.Now.zonedDateTimeISO() instanceof Temporal.ZonedDateTime);

const zone = Temporal.Now.timeZoneId();
const zoned = Temporal.Now.zonedDateTimeISO(zone);
assertEquals(typeof zoned.epochNanoseconds, "bigint");
assertEquals(zoned.timeZoneId, zone);
