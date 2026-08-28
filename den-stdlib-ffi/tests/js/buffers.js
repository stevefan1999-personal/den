import { assert, assertEquals, assertThrows } from "den:assert";
import { grant, open } from "den:ffi";
import { library } from "./probe.js";

const capability = grant();
const fill = { params: ["buffer", "usize"], result: "void" };
const probe = open(library, { fill_bytes: fill }, capability);

// The pointer C receives is the view's first byte, not the buffer's — and it
// is writable, which `as_bytes()`'s `&[u8]` could never have been.
const owner = new ArrayBuffer(12);
probe.fill_bytes(new Uint8Array(owner, 2), 8n);
assertEquals(
  Array.from(new Uint8Array(owner)),
  [0, 0, 1, 2, 3, 4, 5, 6, 7, 8, 0, 0],
);

// A view over the whole of its own buffer is the same path with a zero offset.
const whole = new Uint8Array(4);
probe.fill_bytes(whole, 4n);
assertEquals(Array.from(whole), [1, 2, 3, 4]);

/// Each hazard has its own mechanism, so each refusal is checked by the reason
/// it gives and not only by its kind.
function refuses(call, because) {
  const error = assertThrows(call);
  assertEquals(error.kind, "BadArgument");
  assert(error.message.includes(because), error.message);
}

// Only a Uint8Array is a `buffer`: another element type has a different stride,
// and a number is not a buffer at all.
for (const wrong of [new Int32Array(4), new Uint16Array(4), 8, null, "bytes"]) {
  refuses(() => probe.fill_bytes(wrong, 4n), "expected a Uint8Array");
}

// A detached buffer has no bytes to lend. This is the one hazard `as_raw()`
// reports by itself.
const donor = new ArrayBuffer(8);
const borrowed = new Uint8Array(donor);
donor.transfer();
refuses(() => probe.fill_bytes(borrowed, 4n), "detached");

// The two hazards rquickjs cannot report, each refused by its own read: a
// shared store another agent may write while C holds the address, and a
// resizable one whose bytes move out from under it.
const shared = new Uint8Array(new SharedArrayBuffer(8));
refuses(() => probe.fill_bytes(shared, 4n), "SharedArrayBuffer");

const growable = new Uint8Array(new ArrayBuffer(8, { maxByteLength: 16 }));
assertEquals(growable.buffer.resizable, true, "den can still build one");
refuses(() => probe.fill_bytes(growable, 4n), "resizable ArrayBuffer");

// Neither refusal above is a property read a script can talk den out of: the
// shared one is a class-id check on the buffer itself (`JS_IsArrayBuffer`), so
// a replaced global and a `Symbol.hasInstance` of one's own both change
// nothing.
const realShared = SharedArrayBuffer;
globalThis.SharedArrayBuffer = class {};
refuses(
  () => probe.fill_bytes(new Uint8Array(new realShared(8)), 4n),
  "SharedArrayBuffer",
);
Object.defineProperty(realShared, Symbol.hasInstance, { value: () => false });
refuses(
  () => probe.fill_bytes(new Uint8Array(new realShared(8)), 4n),
  "SharedArrayBuffer",
);
globalThis.SharedArrayBuffer = realShared;

// Marshalling a *later* argument runs JS, so the address den took for this
// buffer has to be taken again once the whole list is marshalled. Here the
// struct's field getter detaches the store in between: the call is refused,
// rather than made through an address JS has already freed.
const late = open(library, {
  fill_then_padded: {
    params: ["buffer", "usize", { struct: { b: "i8", n: "i32" } }],
    result: "void",
  },
}, capability);

const stable = new Uint8Array(8);
late.fill_then_padded(stable, 8n, { b: 1, n: 2 });
assertEquals(Array.from(stable), [3, 3, 3, 3, 3, 3, 3, 3]);

const stolen = new ArrayBuffer(8);
const doomed = new Uint8Array(stolen);
refuses(
  () =>
    late.fill_then_padded(doomed, 8n, {
      b: 1,
      get n() {
        stolen.transfer();
        return 2;
      },
    }),
  "detached",
);
late[Symbol.dispose]();

// Zero-copy is a synchronous capability: copying in and silently not copying
// back is the failure mode den refuses to have. A `buffer` argument and
// `nonblocking: true` are refused together, naming the symbol.
const rejected = assertThrows(
  () => open(library, { fill_bytes: { ...fill, nonblocking: true } }, capability),
);
assertEquals(rejected.kind, "Schema");
assertEquals(rejected.symbol, "fill_bytes");

probe[Symbol.dispose]();
assertEquals(
  assertThrows(() => probe.fill_bytes(new Uint8Array(4), 4n)).kind,
  "Closed",
);
