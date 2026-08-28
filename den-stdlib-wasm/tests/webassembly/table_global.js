import { assert, assertEquals, assertThrows } from "den:assert";

const WASM = globalThis.wat2wasm(`
    (module
      (table (export "table") 1 funcref)
      (global (export "counter") (mut i32) (i32.const 7)))
`);
const { table, counter } = (await WebAssembly.instantiate(WASM)).instance.exports;
const lengthBeforeGrow = table.length;
const previousLength = table.grow(2);
table.set(0, null);
assert(table instanceof WebAssembly.Table);
assertEquals(lengthBeforeGrow, 1);
assertEquals(previousLength, 1);
assertEquals(table.length, 3);
assertEquals(table.get(0), null);
assertThrows(() => table.get(99), RangeError);
assert(counter instanceof WebAssembly.Global);
counter.value = 11;
assertEquals(counter.value, 11);
