const {
  assertEquals,
  assertNotEquals,
  assertStrictEquals,
  assertNotStrictEquals,
  equal,
} = await import("den:assert");

assertEquals(1, 1);
assertEquals("a", "a");
assertEquals({ a: 1, b: [2, 3] }, { a: 1, b: [2, 3] });
assertEquals(Number.NaN, Number.NaN);
assertNotEquals(1, 2);
assertStrictEquals(1, 1);
assertNotStrictEquals(1, "1");
if (!equal({ x: 1 }, { x: 1 })) throw new Error("equal objects");
if (equal({ x: 1 }, { x: 2 })) throw new Error("unequal objects");
