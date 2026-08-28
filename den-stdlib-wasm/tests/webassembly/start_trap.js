import { assertEquals } from "den:assert";

const WASM = globalThis.wat2wasm(`(module (func $boom unreachable) (start $boom))`);
let name = "no rejection";
try {
  new WebAssembly.Instance(new WebAssembly.Module(WASM));
} catch (error) {
  name = error.name;
}
assertEquals(name, "RuntimeError");
