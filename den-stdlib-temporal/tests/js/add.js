import { assertEquals } from "den:assert";

const date = Temporal.PlainDate.from("2025-01-15");
assertEquals(date.add({ days: 1 }).toString(), "2025-01-16");
assertEquals(date.subtract({ days: 14 }).toString(), "2025-01-01");
