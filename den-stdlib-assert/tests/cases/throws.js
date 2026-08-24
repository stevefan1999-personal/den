const { assertThrows, assertRejects, AssertionError } = await import("den:assert");

const thrown = assertThrows(() => {
  throw new TypeError("boom");
}, TypeError, "boom");
if (!(thrown instanceof TypeError)) throw new Error("wrong class");

await assertRejects(() => Promise.reject(new RangeError("nope")), RangeError, "nope");

assertThrows(() => {
  throw new AssertionError("x");
}, AssertionError);
