import { assert, assertEquals, assertThrows } from "den:assert";
import { grant, open, suffix } from "den:ffi";
import { library } from "./probe.js";

const capability = grant();
assert(suffix.startsWith("."), "the platform's shared-library suffix");

const probe = open(library, {
  echo_i8: { params: ["i8"], result: "i8" },
  echo_u8: { params: ["u8"], result: "u8" },
  echo_i16: { params: ["i16"], result: "i16" },
  echo_u16: { params: ["u16"], result: "u16" },
  echo_u32: { params: ["u32"], result: "u32" },
  echo_i64: { params: ["i64"], result: "i64" },
  echo_u64: { params: ["u64"], result: "u64" },
  echo_isize: { params: ["isize"], result: "isize" },
  echo_usize: { params: ["usize"], result: "usize" },
  echo_f32: { params: ["f32"], result: "f32" },
  minus_one_i8: { params: [], result: "i8" },
  negate: { params: ["bool"], result: "bool" },

  // `name` overrides what is passed to dlsym; the key stays the JS property.
  sum: { params: ["i32", "i32"], result: "i32", name: "add" },
  // An `optional` name the library does not export is null, not a failure.
  absent: { params: [], result: "void", optional: true },
  absent_static: { type: "i32", optional: true, name: "no_such_variable" },

  // Static symbols: read once at open(), a property and not a getter.
  version: { type: "i32" },
  ratio: { type: "f64" },
}, capability);

// Every width round-trips at its own boundaries, and one past them is a
// refusal rather than a wrap.
assertEquals(probe.echo_i8(-128), -128);
assertEquals(probe.echo_i8(127), 127);
assertEquals(assertThrows(() => probe.echo_i8(128)).kind, "Range");
assertEquals(assertThrows(() => probe.echo_i8(-129)).kind, "Range");
assertEquals(probe.echo_u8(255), 255);
assertEquals(assertThrows(() => probe.echo_u8(-1)).kind, "Range");
assertEquals(probe.echo_i16(-32768), -32768);
assertEquals(probe.echo_u16(65535), 65535);
assertEquals(probe.echo_u32(4294967295), 4294967295);
assertEquals(assertThrows(() => probe.echo_u32(4294967296)).kind, "Range");
assertEquals(assertThrows(() => probe.echo_i8(1.5)).kind, "Range");
assertEquals(assertThrows(() => probe.echo_i8(NaN)).kind, "Range");

// THE assertion of doc 09 fact 12: `BigInt::to_i64` reports 2n**64n - 1n as
// -1 and 2n**70n as 0, and quickjs-ng's `BigInt.asUintN` cannot be the guard
// because it is wrong for every bit width that is a multiple of 32. Only the
// decimal-string path survives both, and only this equality proves it.
assertEquals(probe.echo_u64(18446744073709551615n), 18446744073709551615n);
assertEquals(probe.echo_i64(-9223372036854775808n), -9223372036854775808n);
assertEquals(probe.echo_i64(9223372036854775807n), 9223372036854775807n);
assertEquals(probe.echo_u64(0n), 0n);
assertEquals(assertThrows(() => probe.echo_u64(2n ** 70n)).kind, "Range");
assertEquals(assertThrows(() => probe.echo_u64(-1n)).kind, "Range");
assertEquals(assertThrows(() => probe.echo_i64(9223372036854775808n)).kind, "Range");

// A `number` cannot hold every 64-bit value, so it is refused outright rather
// than widened into one.
assertEquals(assertThrows(() => probe.echo_u64(1)).kind, "BadArgument");
assertEquals(assertThrows(() => probe.echo_i8(1n)).kind, "BadArgument");

assertEquals(probe.echo_isize(-1n), -1n);
assertEquals(probe.echo_usize(4096n), 4096n);

// `f32` narrows silently, exactly as C does at the same call.
assertEquals(probe.echo_f32(0.5), 0.5);
assertEquals(probe.echo_f32(0.1), Math.fround(0.1));

// A return narrower than a register comes back whole and signed.
assertEquals(probe.minus_one_i8(), -1);

assertEquals(probe.negate(true), false);
assertEquals(probe.negate(false), true);
assertEquals(assertThrows(() => probe.negate(1)).kind, "BadArgument");

assertEquals(probe.sum(35, 7), 42);
assertEquals(probe.absent, null);
assertEquals(probe.absent_static, null);
assertEquals(probe.version, 3);
assertEquals(probe.ratio, 0.5);

probe[Symbol.dispose]();
