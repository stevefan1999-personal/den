import { assert, assertThrows } from "den:assert";

const u32 = new Uint32Array(8);
const returned = crypto.getRandomValues(u32);
assert(returned === u32);
assert(u32.some((word) => word !== 0));
assertThrows(() => crypto.getRandomValues([]), TypeError, "not a typed array");
assertThrows(() => crypto.getRandomValues({}), TypeError, "not a typed array");
