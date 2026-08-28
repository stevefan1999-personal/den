import { assertEquals } from "den:assert";

const WASM = globalThis.wat2wasm(`
    (module
      (import "env" "log" (func $log (param i32 i32) (result i32)))
      (func (export "run") (param i32 i32) (result i32)
        local.get 0 local.get 1 call $log))
`);
const sentinel = new RangeError("from the import");
const { instance } = await WebAssembly.instantiate(WASM, {
  env: { log: () => { throw sentinel; } },
});
let thrown = "nothing thrown";
try {
  instance.exports.run(1, 2);
} catch (error) {
  thrown = error === sentinel ? "same" : error.name;
}
assertEquals(thrown, "same");
