import { assertEquals } from "den:assert";

const WASM = globalThis.wat2wasm(`(module (tag (export "t") (param i32)))`);
const { instance } = await WebAssembly.instantiate(WASM);
const tag = instance.exports.t;
assertEquals(tag instanceof WebAssembly.Tag, true);
assertEquals(tag.type().parameters.join(), "i32");
