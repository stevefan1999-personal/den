import { assertEquals, assertThrows } from "den:assert";
import { grant, open } from "den:ffi";
import { library } from "./probe.js";

const capability = grant();
const add = { params: ["i32", "i32"], result: "i32" };

const missing = assertThrows(() => open(`${library}.nope`, { add }, capability));
assertEquals(missing.kind, "Open");

// The grant is scoped to the fixture directory, so a path outside it is
// refused even though it exists.
const outside = assertThrows(() => open("/proc/self/maps", { add }, capability));
assertEquals(outside.kind, "NotCapable");

// Every symbol is resolved at open(), so a name the library does not export
// is an error that names the schema key.
const absent = assertThrows(
  () => open(library, { definitely_missing: add }, capability),
);
assertEquals(absent.kind, "Symbol");
assertEquals(absent.symbol, "definitely_missing");

// ... and `name` is what dlsym sees, so overriding it with a name the library
// does not export fails the same way, still naming the key.
const renamed = assertThrows(
  () => open(library, { add: { ...add, name: "definitely_missing" } }, capability),
);
assertEquals(renamed.kind, "Symbol");
assertEquals(renamed.symbol, "add");

// Nothing is silently widened or ignored: an unimplemented type, an
// unimplemented key and a malformed entry all throw, naming the symbol.
for (const broken of [
  // A struct is legal, but not an empty one, not one with a `void` field, and
  // not with a key beside `struct`.
  { params: [{ struct: {} }], result: "i32" },
  { params: [{ struct: { x: "void" } }], result: "i32" },
  { params: [{ struct: { x: "i32" }, oops: 1 }], result: "i32" },
  // A callback parameter is legal; a callback that takes a `buffer` is not —
  // C hands the trampoline an address with no length — and neither direction
  // of a callback takes a struct by value.
  { params: [{ callback: { params: ["buffer"], result: "void" } }], result: "i32" },
  {
    params: [{ callback: { params: [{ struct: { x: "i32" } }], result: "void" } }],
    result: "i32",
  },
  { params: [{ callback: { params: [], result: { struct: { x: "i32" } } } }], result: "i32" },
  { params: [{ callback: { params: [], result: "void" }, oops: 1 }], result: "i32" },
  { params: ["i32", "i32"], result: "buffer" },
  { params: ["void"], result: "void" },
  { params: ["i32", "i32"], result: "i32", nonblokcing: true },
  // `nonblocking` is legal now, but not over a borrowed buffer: the bytes are
  // the script's, and the call outlives the call site.
  { params: ["buffer"], result: "i32", nonblocking: true },
  { result: "i32" },
  { params: ["i32", "i32"] },
  // A static and a function are different key sets, so neither accepts the
  // other's keys, and `void` names no bytes to read.
  { type: "i32", params: [] },
  { type: "void" },
  { type: "buffer" },
  { type: "quad" },
  "add",
]) {
  const error = assertThrows(() => open(library, { add: broken }, capability));
  assertEquals(error.kind, "Schema");
  assertEquals(error.symbol, "add");
}
