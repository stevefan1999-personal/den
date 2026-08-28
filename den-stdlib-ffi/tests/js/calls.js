import { assert, assertEquals, assertThrows } from "den:assert";
import { grant, open } from "den:ffi";
import { library } from "./probe.js";

const capability = grant();
assert(capability !== null, "the test engine granted FFI");

const probe = open(library, {
  add: { params: ["i32", "i32"], result: "i32" },
  mul_f64: { params: ["f64", "f64"], result: "f64" },
  no_op: { params: [], result: "void" },
}, capability);

assertEquals(probe.add(35, 7), 42);
assertEquals(probe.mul_f64(1.5, 2.0), 3.0);
assertEquals(probe.no_op(), undefined);

// A number that is not an i32 is refused, not truncated.
assertEquals(assertThrows(() => probe.add(1.5, 1)).kind, "Range");
assertEquals(assertThrows(() => probe.add(2 ** 31, 1)).kind, "Range");
assertEquals(assertThrows(() => probe.add(1)).kind, "BadArgument");
assertEquals(assertThrows(() => probe.add("1", 2)).kind, "BadArgument");

probe[Symbol.dispose]();

// The whole point of the liveness check: a call after dispose throws rather
// than jumping into an unmapped page.
const closed = assertThrows(() => probe.add(1, 1));
assertEquals(closed.kind, "Closed");
assertEquals(closed.name, "FfiError");
assert(closed instanceof Error, "FfiError is an Error");

// Disposing twice is a no-op, not a second dlclose.
probe[Symbol.dispose]();
