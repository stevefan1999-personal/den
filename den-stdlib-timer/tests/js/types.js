const { assertEquals } = await import("den:assert");
const id = setTimeout("x", 0);
assertEquals(typeof clearTimeout, "function");
assertEquals(typeof id, "number");
