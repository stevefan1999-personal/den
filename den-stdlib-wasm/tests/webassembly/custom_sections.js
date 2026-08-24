import { assert, assertEquals } from "den:assert";
import { WASM } from "./add.js";

const module = await WebAssembly.compile(WASM);
const greeting = WebAssembly.Module.customSections(module, "greeting");
const missing = WebAssembly.Module.customSections(module, "absent");
assertEquals(WebAssembly.Module.imports(module).length, 0);
assertEquals(WebAssembly.Module.exports(module).length, 2);
assertEquals(String.fromCharCode(...new Uint8Array(greeting[0])), "hello");
assert(greeting[0] instanceof ArrayBuffer);
assertEquals(missing.length, 0);
