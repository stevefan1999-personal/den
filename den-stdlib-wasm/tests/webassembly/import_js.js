import { assertEquals } from "den:assert";

const WASM = globalThis.wat2wasm(`
    (module
      (import "env" "log" (func $log (param i32 i32) (result i32)))
      (func (export "run") (param i32 i32) (result i32)
        local.get 0 local.get 1 call $log))
`);
let seen = null;
const { instance } = await WebAssembly.instantiate(WASM, {
  env: { log: (left, right) => { seen = [left, right]; return left * right; } },
});
assertEquals(instance.exports.run(6, 7), 42);
assertEquals(seen.join(","), "6,7");
const compiled = await WebAssembly.compile(WASM);
assertEquals(
  JSON.stringify([WebAssembly.Module.imports(compiled), WebAssembly.Module.exports(compiled)]),
  '[[{"module":"env","name":"log","kind":"function"}],[{"name":"run","kind":"function"}]]',
);
