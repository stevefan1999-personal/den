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

// Zero-copy is a synchronous capability: copying in and silently not copying
// back is the failure mode den refuses to have. `nonblocking` is not a legal
// key at all yet, so the whole entry is refused, naming the symbol.
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
