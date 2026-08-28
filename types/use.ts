// The check for `den-ffi.d.ts`: `tsc -p types` must exit 0, which proves both
// that the inference works and that every `@ts-expect-error` below is
// consumed — an unused one is itself an error (TS2578).
//
// Nothing here runs; the runtime implements everything but `struct`, which it
// refuses at `open()`.
import {
  type Callback,
  callback,
  FfiGrant,
  grant,
  open,
  type Pointer,
  ptr,
  suffix,
} from "den:ffi";

declare const capability: FfiGrant;

// No `as const` anywhere: `const S` on `open` keeps the literal narrow.
const lib = open(`./libprobe${suffix}`, {
  add: { params: ["i32", "i32"], result: "i32" },
  scale: {
    params: [{ struct: { x: "i32", y: "i32" } }, "i32"],
    result: { struct: { x: "i32", y: "i32" } },
  },
  hash: { params: ["usize"], result: "u64", nonblocking: true },
  digest: { params: ["buffer", "usize"], result: "u64" },
  apply: {
    params: [{ callback: { params: ["i32"], result: "i32" } }, "i32"],
    result: "i32",
    nonblocking: true,
  },
  // The same C function without `nonblocking`, which is what makes its
  // callback slot uninhabited.
  applySync: {
    name: "apply",
    params: [{ callback: { params: ["i32"], result: "i32" } }, "i32"],
    result: "i32",
  },
  alloc: { params: ["usize"], result: "pointer" },
  maybe: { params: [], result: "void", optional: true },
  version: { type: "i32" },
  sum: { params: ["i32", "i32"], result: "i32", name: "add" },
}, capability);

const sum: number = lib.add(1, 2);
const point: { x: number; y: number } = lib.scale({ x: 1, y: 2 }, 3);
const hashed: Promise<bigint> = lib.hash(4n);
const digest: bigint = lib.digest(new Uint8Array(4), 4n);
const version: number = lib.version;
const address: Pointer | null = lib.alloc(8n);
lib.maybe?.();
lib[Symbol.dispose]();
const renamed: number = lib.sum(1, 2);
void [sum, point, hashed, digest, version, address, renamed];

// The `ptr` namespace: opaque in, plain data out.
if (address !== null) {
  const bits: bigint = ptr.value(address);
  const same: boolean = ptr.equals(address, null);
  const bytes: Uint8Array<ArrayBuffer> = ptr.view(address, 8);
  const text: string = ptr.cstring(address, 64);
  void [bits, same, bytes, text];
}

using doubler = callback({ params: ["i32"], result: "i32" }, (n) => n * 2);
const applied: Promise<number> = lib.apply(doubler, 21);
const code: Pointer = doubler.pointer;
void [applied, code];

// @ts-expect-error — wrong arity.
lib.add(1);
// @ts-expect-error — `i32` is a number, not a string.
lib.add("1", 2);
// @ts-expect-error — `z` is not a field of the declared struct.
lib.scale({ x: 1, y: 2, z: 3 }, 3);
// @ts-expect-error — `usize` demands a bigint.
lib.hash(4);
// @ts-expect-error — a `nonblocking` result is a Promise.
const notAwaited: bigint = lib.hash(4n);
open("./libprobe.so", {
  f: { params: ["buffer"], result: "void", nonblocking: true },
  // @ts-expect-error — a `buffer` may not be lent across a nonblocking call.
}, capability).f(new Uint8Array(1));
// @ts-expect-error — a pointer is not a number.
const notANumber: number = lib.alloc(8n);
// @ts-expect-error — an `optional` symbol may be null.
lib.maybe();
// @ts-expect-error — a bare function is not a `Callback` handle.
lib.apply((n: number) => n * 2, 1);
// @ts-expect-error — a callback may only be handed to a `nonblocking` symbol.
lib.applySync(doubler, 21);
// @ts-expect-error — the signature brand: this handle is the wrong shape.
lib.apply(wrongSignature, 1);
// @ts-expect-error — the implementation is typed against its own definition.
callback({ params: ["i32"], result: "i32" }, (text: string) => text.length);
// @ts-expect-error — a callback is handed addresses, so it cannot take a buffer.
callback({ params: ["buffer"], result: "void" }, () => {});
// @ts-expect-error — `buffer` is an argument type, not a static's type.
open("./libprobe.so", { v: { type: "buffer" } }, capability);
// @ts-expect-error — `buffer` is not a result type.
open("./libprobe.so", { f: { params: [], result: "buffer" } }, capability);
// @ts-expect-error — the grant is unforgeable.
open("./libprobe.so", { f: { params: [], result: "void" } }, { roots: [] });

// @ts-expect-error — `view`'s length is mandatory.
ptr.view(address!);
// @ts-expect-error — an address cannot be made from an integer.
ptr.value(0n);

declare const wrongSignature: Callback<readonly ["f64", "f64"], "f64">;
void [notAwaited, notANumber, grant];
