import { assertEquals } from "den:assert";
import { wasiImports } from "den:wasm";

const CALLS_WASI = globalThis.wat2wasm(`
    (module
      (import "wasi_snapshot_preview1" "environ_sizes_get"
              (func $sizes (param i32 i32) (result i32)))
      (memory (export "memory") 1)
      (func (export "run") (result i32) (call $sizes (i32.const 0) (i32.const 8))))
`);

let outcome = "not reached";
try {
  const { instance } = await WebAssembly.instantiate(CALLS_WASI, {
    wasi_snapshot_preview1: wasiImports(),
  });
  outcome = `errno ${instance.exports.run()}`;
} catch (error) {
  outcome = `${error.name}: ${error.message}`;
}
assertEquals(outcome, "errno 0");
