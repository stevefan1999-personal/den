import { assert, assertEquals } from "den:assert";
import { WASM } from "./add.js";

const result = await WebAssembly.instantiate(WASM);
assert(result.module instanceof WebAssembly.Module);
assert(result.instance instanceof WebAssembly.Instance);
assertEquals(result.instance.exports.add(20, 22), 42);
