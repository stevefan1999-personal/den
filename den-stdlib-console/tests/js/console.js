import { assertEquals } from "den:assert";
console.log("integration", { nested: [1, 2] }, 3);
console.error("to stderr");
assertEquals(typeof console.log, "function");
