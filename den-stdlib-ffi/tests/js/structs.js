import { assertEquals, assertThrows } from "den:assert";
import { grant, layout, open } from "den:ffi";
import { library } from "./probe.js";

const capability = grant();

const point = { struct: { x: "i32", y: "i32" } };
const triple = { struct: { a: "f64", b: "f64", c: "f64" } };
const padded = { struct: { b: "i8", n: "i32" } };
const nested = { struct: { origin: point, z: "i32" } };

const probe = open(library, {
  alignof_double: { params: [], result: "u32" },
  point_scale: { params: [point, "i32"], result: point },
  triple_sum: { params: [triple, "f64"], result: triple },
  padded_sum: { params: [padded], result: "i32" },
  nested_sum: { params: [nested], result: "i32" },
  // A struct crosses a worker thread like any other value: the cells are
  // den's own bytes and the CIF is rebuilt there from the same signature.
  triple_later: {
    params: [triple, "f64"],
    result: triple,
    nonblocking: true,
    name: "triple_sum",
  },
  base_point: { type: point },
}, capability);

// The calibration knob (§5.2): den publishes the layout it computed, so a
// script can assert it against the header it is binding. The arithmetic itself
// is already checked against libffi's at open() — this is the part a human
// checks against a struct definition.
assertEquals(layout(padded), { size: 8, align: 4, offsets: { b: 0, n: 4 } });
assertEquals(layout(point), { size: 8, align: 4, offsets: { x: 0, y: 4 } });
assertEquals(layout(triple), {
  size: 24,
  align: probe.alignof_double(),
  offsets: { a: 0, b: 8, c: 16 },
});
assertEquals(layout(nested), {
  size: 12,
  align: 4,
  offsets: { origin: 0, z: 8 },
});

// `layout` describes a struct; a scalar has no fields to describe, and an
// empty struct is not a C type.
assertEquals(assertThrows(() => layout("i32")).kind, "Schema");
assertEquals(assertThrows(() => layout({ struct: {} })).kind, "Schema");

// Eight bytes: SysV register class, in and out.
assertEquals(probe.point_scale({ x: 3, y: 4 }, 3), { x: 9, y: 12 });

// Twenty-four bytes: MEMORY class, so libffi passes a hidden `sret` pointer to
// a buffer den owns and the result never travels in a register. A design that
// only handles register-class structs passes the case above and corrupts the
// stack here.
assertEquals(probe.triple_sum({ a: 1, b: 2, c: 3 }, 0.5), {
  a: 1.5,
  b: 2.5,
  c: 3.5,
});
assertEquals(await probe.triple_later({ a: 1, b: 2, c: 3 }, 1), {
  a: 2,
  b: 3,
  c: 4,
});

// Padding: this compiler put `n` at offset 4, and den has to have computed the
// same or the sum is not 42.
assertEquals(probe.padded_sum({ b: 3, n: 39 }), 42);
assertEquals(probe.nested_sum({ origin: { x: 20, y: 15 }, z: 7 }), 42);

// A static struct is read once at open(), exactly like a scalar one.
assertEquals(probe.base_point, { x: 2, y: 3 });

// A field the object does not carry is named, never invented as a zero.
const missing = assertThrows(() => probe.point_scale({ x: 1 }, 2));
assertEquals(missing.kind, "Schema");
assertEquals(missing.symbol, "y");

// Fields are marshalled like any other value of their type, refusals included.
assertEquals(assertThrows(() => probe.padded_sum({ b: 128, n: 0 })).kind, "Range");
assertEquals(assertThrows(() => probe.point_scale(7, 2)).kind, "BadArgument");

probe[Symbol.dispose]();

// A struct field is an ordinary property read, so a getter runs arbitrary JS
// in the middle of marshalling — including disposing the library the call is
// about to jump into. That is a typed throw, not a segfault.
const doomed = open(library, {
  padded_sum: { params: [padded], result: "i32" },
}, capability);
const detonator = {
  get b() {
    doomed[Symbol.dispose]();
    return 1;
  },
  n: 41,
};
assertEquals(assertThrows(() => doomed.padded_sum(detonator)).kind, "Closed");
