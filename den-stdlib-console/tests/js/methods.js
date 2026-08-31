import { assertEquals } from "den:assert";

assertEquals(typeof console.debug, "function");
assertEquals(typeof console.log, "function");
assertEquals(typeof console.info, "function");
assertEquals(typeof console.dir, "function");
assertEquals(typeof console.warn, "function");
assertEquals(typeof console.error, "function");
assertEquals(typeof console.trace, "function");
assertEquals(typeof console.assert, "function");
console.debug("debug", 1);
console.warn("warn", { ok: true });
console.error("error", ["nested"]);
console.assert(true, "must not print");
console.assert(false, "assertion %s", "failed");
console.trace("trace-frame");
console.log("%i %f %c ignore", "9", "1.5", "color:red");
console.log("%", "dangling");
console.log("%q", "unknown");
