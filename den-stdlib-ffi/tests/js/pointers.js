import { assert, assertEquals, assertThrows } from "den:assert";
import { grant, open, ptr } from "den:ffi";
import { library } from "./probe.js";

const capability = grant();

const probe = open(library, {
  cell_address: { params: [], result: "pointer" },
  read_i32: { params: ["pointer"], result: "i32" },
  null_pointer: { params: [], result: "pointer" },
  hello: { params: [], result: "pointer" },
  unterminated: { params: [], result: "pointer" },
}, capability);

const cell = probe.cell_address();
assert(cell !== null, "a non-null C pointer is a Pointer");

// A pointer round-trips through C unchanged.
assertEquals(probe.read_i32(cell), 7);

// The null pointer is JS `null`, so a script never holds a Pointer it must
// not dereference.
assertEquals(probe.null_pointer(), null);

// Two Pointer objects naming the same address are distinct JS values, which
// is what `ptr.equals` is for.
assert(cell !== probe.cell_address(), "each call mints a fresh handle");
assert(ptr.equals(cell, probe.cell_address()), "same address");
assert(ptr.equals(null, null), "null equals null");
assert(!ptr.equals(cell, null), "an address is not null");
assertEquals(typeof ptr.value(cell), "bigint");
assertEquals(ptr.value(cell), ptr.value(probe.cell_address()));

// An address is opaque: there is no way to make one from an integer, so a
// number where a pointer belongs is refused rather than dereferenced.
assertEquals(assertThrows(() => probe.read_i32(123)).kind, "BadArgument");
assertEquals(assertThrows(() => ptr.value(0n)).kind, "BadArgument");
assertEquals(assertThrows(() => ptr.view(1n, 4)).kind, "BadArgument");

// `view`'s length is mandatory and is NOT a bounds check — den cannot know
// how large a pointee is. What it buys is that every read *after* the view is
// bounds checked by the engine.
const bytes = ptr.view(probe.hello(), 5);
assertEquals(bytes.length, 5);
assertEquals(Array.from(bytes), [104, 101, 108, 108, 111]);
assertEquals(bytes[5], undefined);
assertEquals(assertThrows(() => ptr.view(cell)).kind, "BadArgument");

assertEquals(ptr.cstring(probe.hello()), "hello");
assertEquals(ptr.cstring(probe.hello(), 16), "hello");

// A bounded scan that finds no NUL is an error, never a truncated string.
assertEquals(assertThrows(() => ptr.cstring(probe.unterminated(), 4)).kind, "Range");

probe[Symbol.dispose]();

// Provenance: every Pointer carries the library that produced it, so a
// dereference after dispose is a typed throw and not a read of unmapped pages.
assertEquals(assertThrows(() => ptr.view(cell, 4)).kind, "Closed");
assertEquals(assertThrows(() => ptr.cstring(cell, 4)).kind, "Closed");
