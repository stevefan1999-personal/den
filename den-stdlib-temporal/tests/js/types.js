import { assert, assertEquals, assertThrows } from "den:assert";

const time = new Temporal.PlainTime(13, 45, 30);
assert(time instanceof Temporal.PlainTime);
assertEquals(time.hour, 13);
assertEquals(time.minute, 45);
assertEquals(Temporal.PlainTime.from("13:45:30").second, 30);

const date = Temporal.PlainDate.from("2025-03-03");
assertEquals(date.month, 3);
assertEquals(date.day, 3);
assertEquals(date.calendarId, "iso8601");
assertEquals(Temporal.PlainDate.compare("2025-01-01", "2025-01-02"), -1);
assertEquals(Temporal.PlainDate.compare(date, date), 0);

const dateTime = new Temporal.PlainDateTime(2025, 3, 3, 12, 30);
assertEquals(dateTime.hour, 12);
assertEquals(dateTime.monthCode, "M03");

assertEquals(Temporal.PlainYearMonth.from("2025-03").year, 2025);
const monthDay = new Temporal.PlainMonthDay(3, 14);
assertEquals(monthDay.monthCode, "M03");
assertEquals(monthDay.day, 14);

assertEquals(Temporal.Duration.from({ hours: 2, minutes: 30 }).toString(), "PT2H30M");
assertThrows(() => Temporal.PlainDate.from("not-a-date"));
