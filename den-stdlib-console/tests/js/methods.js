import { assertEquals } from "den:assert";

assertEquals(typeof console.debug, "function");
assertEquals(typeof console.log, "function");
assertEquals(typeof console.warn, "function");
assertEquals(typeof console.error, "function");
console.debug("debug", 1);
console.warn("warn", { ok: true });
console.error("error", ["nested"]);
