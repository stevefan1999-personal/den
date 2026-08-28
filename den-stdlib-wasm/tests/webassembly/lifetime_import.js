import { assertEquals } from "den:assert";

const WASM = globalThis.wat2wasm(`
    (module
      (import "env" "twice" (func $twice (param i32) (result i32)))
      (func (export "run") (param i32) (result i32) local.get 0 call $twice))
`);
const { instance } = await WebAssembly.instantiate(WASM, {
  env: { twice: (value) => value * 2 },
});
assertEquals(instance.exports.run(21), 42);
