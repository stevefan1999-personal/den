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

// Nothing is silently widened or ignored: an unimplemented type, an
// unimplemented key and a malformed entry all throw, naming the symbol.
for (const broken of [
  { params: ["u64"], result: "i32" },
  { params: [{ struct: { x: "i32" } }], result: "i32" },
  { params: ["i32", "i32"], result: "buffer" },
  { params: ["void"], result: "void" },
  { params: ["i32", "i32"], result: "i32", nonblocking: true },
  { params: ["i32", "i32"], result: "i32", optional: true },
  { result: "i32" },
  { params: ["i32", "i32"] },
  "add",
]) {
  const error = assertThrows(() => open(library, { add: broken }, capability));
  assertEquals(error.kind, "Schema");
  assertEquals(error.symbol, "add");
}
