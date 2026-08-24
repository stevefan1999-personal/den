import { assert, assertEquals } from "den:assert";

const start = performance.now();
assertEquals(typeof start, "number");
assert(Number.isFinite(start));
assert(start >= 0 && start < 60000);
let later = start;
const deadline = Date.now() + 50;
while (later === start && Date.now() < deadline) later = performance.now();
assert(later > start);
assert(performance.now() >= later);
assertEquals(typeof performance.timeOrigin, "number");
assert(performance.timeOrigin > 1e12);
assert(Math.abs(Date.now() - performance.timeOrigin) < 60000);
assertEquals(Object.prototype.toString.call(performance), "[object Performance]");
