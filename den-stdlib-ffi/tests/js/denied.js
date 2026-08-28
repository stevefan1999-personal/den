import { assertEquals, assertThrows } from "den:assert";
import { grant, open } from "den:ffi";
import { library } from "./probe.js";

const schema = { add: { params: ["i32", "i32"], result: "i32" } };

// No grant in this realm: `grant()` says so, and `open` refuses.
assertEquals(grant(), null);
assertEquals(assertThrows(() => open(library, schema, null)).kind, "NotCapable");
assertEquals(assertThrows(() => open(library, schema)).kind, "NotCapable");

// A grant is a class instance, not a shape: a forged one is still no grant.
assertEquals(
  assertThrows(() => open(library, schema, { roots: ["/"] })).kind,
  "NotCapable",
);
