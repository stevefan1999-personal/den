import { assertEquals } from "den:assert";

const WASM = globalThis.wat2wasm(`
    (module
      (import "env" "fill" (func $fill (param i32 i32)))
      (memory (export "mem") 1)
      (func (export "run") (param i32 i32)
        local.get 0
        local.get 1
        call $fill)
      (func (export "peek") (param i32) (result i32)
        local.get 0
        i32.load8_u))
`);

const { instance } = await WebAssembly.instantiate(WASM, {
  env: {
    fill: (ptr, value) => {
      new Uint8Array(instance.exports.mem.buffer)[ptr] = value;
    },
  },
});

instance.exports.run(7, 99);
assertEquals(instance.exports.peek(7), 99);

const { instance: offsetInstance } = await WebAssembly.instantiate(WASM, {
  env: {
    fill: (ptr, value) => {
      new Uint8Array(offsetInstance.exports.mem.buffer, ptr, 1)[0] = value;
    },
  },
});
offsetInstance.exports.run(20, 77);
assertEquals(offsetInstance.exports.peek(20), 77);
assertEquals(offsetInstance.exports.peek(0), 0);
