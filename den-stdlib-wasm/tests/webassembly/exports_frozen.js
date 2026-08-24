import { assert, assertEquals } from "den:assert";
import { WASM } from "./add.js";

const { exports } = (await WebAssembly.instantiate(WASM)).instance;
assert(Object.isFrozen(exports));
assertEquals(Object.getPrototypeOf(exports), null);
assert((await WebAssembly.instantiate(WASM)).instance.exports !== exports);
