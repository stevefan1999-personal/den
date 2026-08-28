import { assert, assertEquals } from "den:assert";

const WASM = globalThis.wat2wasm(`
    (module
      (memory (export "mem") 1)
      (func (export "peek") (param i32) (result i32) local.get 0 i32.load8_u))
`);
const { mem, peek } = (await WebAssembly.instantiate(WASM)).instance.exports;
const stale = mem.buffer;
const sizeBeforeGrow = stale.byteLength;
new Uint8Array(stale)[3] = 42;
const seenByWasm = peek(3);
const previousPages = mem.grow(1);
assert(mem instanceof WebAssembly.Memory);
assertEquals(sizeBeforeGrow, 65536);
assertEquals(seenByWasm, 42);
assertEquals(previousPages, 1);
assertEquals(stale.byteLength, 0);
assert(mem.buffer !== stale);
assertEquals(mem.buffer.byteLength, 131072);
