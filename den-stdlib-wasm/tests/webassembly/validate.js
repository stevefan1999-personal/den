import { assertEquals } from "den:assert";
import { WASM } from "./add.js";

assertEquals(WebAssembly.validate(WASM), true);
assertEquals(WebAssembly.validate(new Uint8Array([1, 2, 3])), false);
