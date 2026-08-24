import { assertEquals } from "den:assert";
import { WASM } from "./hello.js";

const { nothing, one, pair, add } = (await WebAssembly.instantiate(WASM)).instance.exports;
assertEquals(nothing(), undefined);
assertEquals(one(), 7);
assertEquals(Array.isArray(pair()), true);
assertEquals(pair().join("-"), "1-2");
assertEquals(add(20, 22), 42);
assertEquals(add(20), 20);
assertEquals(add(1, 2, 3), 3);
assertEquals(add.length, 2);
assertEquals(add.name, "3");
