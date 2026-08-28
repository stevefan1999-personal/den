// Phase 5: C calling back into JS from a thread of its own, through the
// mailbox a `nonblocking` call leaves the realm free to service.
//
// Every symbol that takes a `{ callback }` is `nonblocking: true`, because that
// is the only kind den lets a callback reach (§4.7): a synchronous symbol runs
// on the one thread that may run JS, so a callback it makes would have nobody
// to answer it.
import {
  assert,
  assertEquals,
  assertRejects,
  assertThrows,
} from "den:assert";
import { callback, grant, open, ptr } from "den:ffi";
import { library } from "./probe.js";

const capability = grant();

const probe = open(library, {
  apply: {
    params: [{ callback: { params: ["i32"], result: "i32" } }, "i32"],
    result: "i32",
    nonblocking: true,
  },
  // The same C function bound a second time, synchronously, to prove what
  // handing it a callback does.
  apply_sync: {
    name: "apply",
    params: [{ callback: { params: ["i32"], result: "i32" } }, "i32"],
    result: "i32",
  },
  apply_f64: {
    params: [{ callback: { params: ["f64"], result: "f64" } }, "f64"],
    result: "f64",
    nonblocking: true,
  },
  notify: {
    params: [{ callback: { params: ["i32"], result: "void" } }, "i32"],
    result: "void",
    nonblocking: true,
  },
  load_values: { params: ["buffer", "usize"], result: "void" },
  sorted_values: { params: [], result: "pointer" },
  sort_values: {
    params: [{ callback: { params: ["pointer", "pointer"], result: "i32" } }],
    result: "void",
    nonblocking: true,
  },
  read_i32: { params: ["pointer"], result: "i32" },
  call_on_thread: {
    params: [{ callback: { params: ["i32"], result: "i32" } }, "i32"],
    result: "i32",
    nonblocking: true,
  },
}, capability);

// The phase's headline: a JS function reached through a C function pointer, on
// a thread neither den nor the script created. The call runs on a worker, so
// C's trampoline posts to the realm's mailbox and the pump answers it.
const double = callback({ params: ["i32"], result: "i32" }, (n) => n * 2);
assertEquals(await probe.apply(double, 21), 42);

// The same, one thread further out: C spawns its own thread inside the worker,
// fires the callback there and joins. This is the case that hangs for ever
// without a `nonblocking` symbol — the realm can service it because the realm
// is not the thread that is waiting.
assertEquals(await probe.call_on_thread(double, 21), 42);

// A float result is written to the return buffer as itself; an integer
// narrower than a register is widened. Both are the same code path in Rust and
// only C can tell them apart.
using half = callback({ params: ["f64"], result: "f64" }, (x) => x / 2);
assertEquals(await probe.apply_f64(half, 7), 3.5);

// `void`: nothing is written to the return buffer at all.
let seen = 0;
using observe = callback(
  { params: ["i32"], result: "void" },
  (n) => {
    seen = n;
  },
);
assertEquals(await probe.notify(observe, 9), undefined);
assertEquals(seen, 9);

// qsort-shaped: C owns the loop, and every comparison is a mailbox round-trip
// plus a realm wakeup. That is microseconds against a comparator that should
// cost nanoseconds — sort in JS, or sort in C with a C comparator, but not
// across this seam.
const values = new Int32Array([5, 3, 9, 1, 4]);
probe.load_values(new Uint8Array(values.buffer), BigInt(values.byteLength));
using compare = callback(
  { params: ["pointer", "pointer"], result: "i32" },
  (left, right) => probe.read_i32(left) - probe.read_i32(right),
);
await probe.sort_values(compare);
assertEquals(
  Array.from(new Int32Array(
    ptr.view(probe.sorted_values(), values.byteLength).buffer,
  )),
  [1, 3, 4, 5, 9],
);

// A callback that throws does not abort the process and does not unwind
// through C: C is handed the zero value, the exception is reported the way an
// uncaught one is, and the script keeps running.
using explode = callback({ params: ["i32"], result: "i32" }, () => {
  throw new Error("from inside a callback");
});
assertEquals(await probe.apply(explode, 21), 0);

// The refusal that makes all of the above safe. `apply_sync` is the same C
// function; handing it a callback would park the realm inside C with the
// callback's only servant — so den refuses at the call, by name.
const refused = assertThrows(() => probe.apply_sync(double, 21));
assertEquals(refused.kind, "BadArgument");
assert(
  refused.message.includes("nonblocking"),
  "the refusal names the word that fixes it",
);

// The handle's signature is checked against the slot it is passed to: a
// mismatch would have libffi read the wrong registers, which no layer can
// diagnose after the fact. A nonblocking symbol reports it the way an async
// function reports anything: by rejecting.
assertEquals(
  (await assertRejects(() => probe.apply(half, 21))).kind,
  "BadArgument",
);
assertEquals(
  (await assertRejects(() => probe.apply((n) => n, 21))).kind,
  "BadArgument",
);

// The raw address, for a C API that takes a bare `pointer`.
assert(ptr.value(double.pointer) !== 0n, "a callback has a real code address");

// Disposal drops the JS function and ends the pump. The trampoline stays
// mapped — C may still hold the address and den cannot know — but every
// JS-side dispatch is now a typed throw.
double[Symbol.dispose]();
double[Symbol.dispose]();
assertEquals(assertThrows(() => double.pointer).kind, "Closed");
assertEquals((await assertRejects(() => probe.apply(double, 21))).kind, "Closed");

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

// And what a nonblocking symbol cannot take: a `buffer` lends the script's own
// bytes for the length of one call, and this call outlives its call site.
assertEquals(
  assertThrows(() =>
    open(library, {
      load_values: {
        params: ["buffer", "usize"],
        result: "void",
        nonblocking: true,
      },
    }, capability)
  ).kind,
  "Schema",
);

probe[Symbol.dispose]();
