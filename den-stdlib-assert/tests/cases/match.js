import {
  assert,
  assertFalse,
  assertExists,
  assertMatch,
  assertNotMatch,
  assertStringIncludes,
  assertArrayIncludes,
  assertObjectMatch,
  assertInstanceOf,
  assertNotInstanceOf,
  assertGreater,
  assertGreaterOrEqual,
  assertLess,
  assertLessOrEqual,
  assertAlmostEquals,
} from "den:assert";

assert(1);
assertFalse(0);
assertExists("x");
assertMatch("hello", /ell/);
assertNotMatch("hello", /xyz/);
assertStringIncludes("hello", "ell");
assertArrayIncludes([1, 2, 3], [2]);
assertObjectMatch({ a: 1, b: 2, c: 3 }, { a: 1, c: 3 });
assertInstanceOf(new Error("x"), Error);
assertNotInstanceOf(1, Error);
assertGreater(3, 1);
assertGreaterOrEqual(2, 2);
assertLess(1, 3);
assertLessOrEqual(2, 2);
assertAlmostEquals(1, 1.0000000001);
