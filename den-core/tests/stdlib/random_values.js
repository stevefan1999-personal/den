const { assertEquals, assert } = await import("den:assert");
const array = new Uint8Array(64);
const returned = crypto.getRandomValues(array);
assert(returned === array);
assertEquals(array.length, 64);
assert(array.some((byte) => byte !== 0));
