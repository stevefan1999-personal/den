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
