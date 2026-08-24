import { assertEquals } from "den:assert";
const resolved = await new Promise((resolve) => setTimeout(() => resolve("fired"), 1));
assertEquals(resolved, "fired");
