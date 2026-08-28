// Phase 4: C calling back into JS, on den's own thread.
import { assert, assertEquals, assertThrows } from "den:assert";
import { callback, grant, open, ptr } from "den:ffi";
import { library } from "./probe.js";

const capability = grant();

const probe = open(library, {
  apply: {
    params: [{ callback: { params: ["i32"], result: "i32" } }, "i32"],
    result: "i32",
  },
  apply_f64: {
    params: [{ callback: { params: ["f64"], result: "f64" } }, "f64"],
    result: "f64",
  },
  notify: {
    params: [{ callback: { params: ["i32"], result: "void" } }, "i32"],
    result: "void",
  },
  sort_i32: {
    params: ["buffer", "usize", {
      callback: { params: ["pointer", "pointer"], result: "i32" },
    }],
    result: "void",
  },
  read_i32: { params: ["pointer"], result: "i32" },
  call_on_thread: {
    params: [{ callback: { params: ["i32"], result: "i32" } }, "i32"],
    result: "i32",
  },
}, capability);

// The phase's headline: a JS function reached through a C function pointer.
const double = callback({ params: ["i32"], result: "i32" }, (n) => n * 2);
assertEquals(probe.apply(double, 21), 42);

// A float result is written to the return buffer as itself; an integer
// narrower than a register is widened. Both are the same code path in Rust and
// only C can tell them apart.
using half = callback({ params: ["f64"], result: "f64" }, (x) => x / 2);
assertEquals(probe.apply_f64(half, 7), 3.5);

// `void`: nothing is written to the return buffer at all.
let seen = 0;
using observe = callback(
  { params: ["i32"], result: "void" },
  (n) => {
    seen = n;
  },
);
assertEquals(probe.notify(observe, 9), undefined);
assertEquals(seen, 9);

// qsort-shaped: C owns the loop, and every comparison is a re-entrant JS call
// that itself calls back into C through `read_i32`. Once phase 5 lands, a
// comparator like this costs a mailbox round-trip per comparison — sort in JS
// or sort in C, not across the seam.
const values = new Int32Array([5, 3, 9, 1, 4]);
using compare = callback(
  { params: ["pointer", "pointer"], result: "i32" },
  (left, right) => probe.read_i32(left) - probe.read_i32(right),
);
probe.sort_i32(new Uint8Array(values.buffer), BigInt(values.length), compare);
assertEquals(Array.from(values), [1, 3, 4, 5, 9]);

// A callback that throws does not abort the process and does not unwind
// through C: C is handed the zero value, the exception is reported the way an
// uncaught one is, and the script keeps running.
using explode = callback({ params: ["i32"], result: "i32" }, () => {
  throw new Error("from inside a callback");
});
assertEquals(probe.apply(explode, 21), 0);

// A thread C created cannot reach the realm: no JS value in the process may
// be touched from it. This phase writes the zero value and one line to stderr
// naming the signature — a loud wrong answer rather than a silent one. Phase 5
// replaces it with a mailbox, and this becomes 42.
assertEquals(probe.call_on_thread(double, 21), 0);

// The handle's signature is checked against the slot it is passed to: a
// mismatch would have libffi read the wrong registers, which no layer can
// diagnose after the fact.
assertEquals(assertThrows(() => probe.apply(half, 21)).kind, "BadArgument");
assertEquals(assertThrows(() => probe.apply((n) => n, 21)).kind, "BadArgument");

// The raw address, for a C API that takes a bare `pointer`.
assert(ptr.value(double.pointer) !== 0n, "a callback has a real code address");

// Disposal drops the JS function. The trampoline stays mapped — C may still
// hold the address and den cannot know — but every JS-side dispatch is now a
// typed throw.
double[Symbol.dispose]();
double[Symbol.dispose]();
assertEquals(assertThrows(() => double.pointer).kind, "Closed");
assertEquals(assertThrows(() => probe.apply(double, 21)).kind, "Closed");

// What a callback cannot be: C hands the trampoline a bare address, so there
// is no length to build a `Uint8Array` from.
assertEquals(
  assertThrows(() => callback({ params: ["buffer"], result: "void" }, () => {}))
    .kind,
  "Schema",
);
assertEquals(
  assertThrows(() =>
    callback({ params: [], result: "void", nonblocking: true }, () => {})
  ).kind,
  "Schema",
);

probe[Symbol.dispose]();
