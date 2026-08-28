// The check for `den-ffi.d.ts`: `tsc -p types` must exit 0, which proves both
// that the inference works and that every `@ts-expect-error` below is
// consumed — an unused one is itself an error (TS2578).
//
// Nothing here runs; the phase-1 runtime implements only the `i32` / `f64` /
// `void` corner of this surface.
import {
  type Callback,
  FfiGrant,
  grant,
  open,
  type Pointer,
} from "den:ffi";

declare const capability: FfiGrant;

// No `as const` anywhere: `const S` on `open` keeps the literal narrow.
const lib = open("./libprobe.so", {
  add: { params: ["i32", "i32"], result: "i32" },
  scale: {
    params: [{ struct: { x: "i32", y: "i32" } }, "i32"],
    result: { struct: { x: "i32", y: "i32" } },
  },
  hash: { params: ["buffer", "usize"], result: "u64", nonblocking: true },
  apply: {
    params: [{ callback: { params: ["i32"], result: "i32" } }, "i32"],
    result: "i32",
  },
  alloc: { params: ["usize"], result: "pointer" },
  maybe: { params: [], result: "void", optional: true },
  version: { type: "i32" },
}, capability);

const sum: number = lib.add(1, 2);
const point: { x: number; y: number } = lib.scale({ x: 1, y: 2 }, 3);
const digest: Promise<bigint> = lib.hash(new Uint8Array(4), 4n);
const version: number = lib.version;
const address: Pointer | null = lib.alloc(8n);
lib.maybe?.();
lib[Symbol.dispose]();
void [sum, point, digest, version, address];

declare const callback: Callback<readonly ["i32"], "i32">;
const applied: number = lib.apply(callback, 21);
void applied;

// @ts-expect-error — wrong arity.
lib.add(1);
// @ts-expect-error — `i32` is a number, not a string.
lib.add("1", 2);
// @ts-expect-error — `z` is not a field of the declared struct.
lib.scale({ x: 1, y: 2, z: 3 }, 3);
// @ts-expect-error — `usize` demands a bigint.
lib.hash(new Uint8Array(4), 4);
// @ts-expect-error — a `nonblocking` result is a Promise.
const notAwaited: bigint = lib.hash(new Uint8Array(4), 4n);
// @ts-expect-error — a pointer is not a number.
const notANumber: number = lib.alloc(8n);
// @ts-expect-error — an `optional` symbol may be null.
lib.maybe();
// @ts-expect-error — a bare function is not a `Callback` handle.
lib.apply((n: number) => n * 2, 1);
// @ts-expect-error — the signature brand: this handle is the wrong shape.
lib.apply(wrongSignature, 1);
// @ts-expect-error — `buffer` is not a result type.
open("./libprobe.so", { f: { params: [], result: "buffer" } }, capability);
// @ts-expect-error — the grant is unforgeable.
open("./libprobe.so", { f: { params: [], result: "void" } }, { roots: [] });

declare const wrongSignature: Callback<readonly ["f64", "f64"], "f64">;
void [notAwaited, notANumber, grant];
