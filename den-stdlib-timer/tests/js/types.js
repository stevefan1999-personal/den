import { assert, assertEquals } from "den:assert";
const id = setTimeout(() => {}, 0);
assertEquals(typeof clearTimeout, "function");
assertEquals(typeof id, "number");
// Browsers hand out ids from 1; 0 must never be a live timer id.
assert(id >= 1, `first timer id was ${id}`);
