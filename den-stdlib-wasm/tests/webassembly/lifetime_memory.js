import { assertEquals } from "den:assert";
import { wat2wasm } from "den:wasm";

const WASM = wat2wasm(`
    (module
      (memory (export "mem") 1)
      (func (export "peek") (param i32) (result i32) local.get 0 i32.load8_u))
`);
const { mem, peek } = (await WebAssembly.instantiate(WASM)).instance.exports;
new Uint8Array(mem.buffer)[0] = 7;
mem.grow(1);
assertEquals(peek(0), 7);
