// cargo run -- examples/wasm.ts
//
// Import attributes `{ type: "bytes" }` hand the file to script as a
// Uint8Array. WebAssembly.instantiate is the JS API on wasmtime.

import { assertEquals } from "den:assert";
import addModule from "./data/add.wasm" with { type: "bytes" };

const { instance } = await WebAssembly.instantiate(addModule);
const add = instance.exports.add as (left: number, right: number) => number;
assertEquals(add(40, 2), 42);
assertEquals(addModule.byteLength, 41);
console.log("wasm add", add(40, 2));
console.log("module bytes", addModule.byteLength);
