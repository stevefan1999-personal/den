const { assertEquals } = await import("den:assert");
const resolved = await new Promise((resolve) => setTimeout(() => resolve("fired"), 1));
assertEquals(resolved, "fired");
