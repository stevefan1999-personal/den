// cargo run -- examples/import-attrs.ts
//
// Typed imports are handled once in the loader: json, text, and bytes.
// Other types fail loading rather than being ignored.

import { assertEquals } from "den:assert";
import greeting from "./data/greeting.json" with { type: "json" };
import text from "./data/greeting.txt" with { type: "text" };
import wasm from "./data/add.wasm" with { type: "bytes" };

assertEquals(greeting.runtime, "den");
assertEquals(greeting.oneLessThan, "deno");
assertEquals(text.trim(), "hello from a text import attribute");
assertEquals([wasm[0], wasm[1], wasm[2], wasm[3]], [0, 97, 115, 109]);
assertEquals(wasm.byteLength, 41);
console.log("json", greeting.runtime, "one less than", greeting.oneLessThan);
console.log("text", text.trim());
console.log("bytes", wasm[0], wasm[1], wasm[2], wasm[3], "len", wasm.byteLength);
