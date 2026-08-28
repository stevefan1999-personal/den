import { assert, assertEquals } from "den:assert";
import * as denWasm from "den:wasm";

const missing = [
  "validate",
  "compile",
  "instantiate",
  "compileStreaming",
  "instantiateStreaming",
  "Module",
  "Instance",
  "Memory",
  "Table",
  "Global",
  "Tag",
  "Exception",
  "CompileError",
  "LinkError",
  "RuntimeError",
].filter((name) => typeof WebAssembly[name] !== "function");
assertEquals(missing.join(","), "");

assertEquals("wat2wasm" in WebAssembly, false);
assertEquals("wat2wasm" in denWasm, false);
assertEquals(typeof globalThis.wat2wasm, "function");
assertEquals("wasiImports" in WebAssembly, false);
if ("wasiImports" in denWasm) {
  assertEquals(typeof denWasm.wasiImports(), "object");
}

const operations = new Set([
  "validate",
  "compile",
  "instantiate",
  "compileStreaming",
  "instantiateStreaming",
]);
const wrong = Object.getOwnPropertyNames(WebAssembly).filter((name) => {
  const descriptor = Object.getOwnPropertyDescriptor(WebAssembly, name);
  if (name === "JSTag") {
    return descriptor.enumerable || !descriptor.configurable || descriptor.set;
  }
  const shouldEnumerate = operations.has(name);
  return descriptor.enumerable !== shouldEnumerate || !descriptor.configurable
    || ("value" in descriptor && !descriptor.writable);
});
const leaked = Object.keys(WebAssembly).filter((name) => !operations.has(name));
assertEquals(wrong.join("|"), "");
assertEquals(leaked.join("|"), "");
assertEquals(String(WebAssembly), "[object WebAssembly]");
assertEquals(
  Object.prototype.toString.call(new WebAssembly.Memory({ initial: 1 })),
  "[object WebAssembly.Memory]",
);

const descriptor = Object.getOwnPropertyDescriptor(WebAssembly, "JSTag");
assert(descriptor !== undefined);
assert(WebAssembly.JSTag instanceof WebAssembly.Tag);
assertEquals(WebAssembly.JSTag, WebAssembly.JSTag);
assertEquals(WebAssembly.JSTag.type().parameters.join(), "externref");
assertEquals(descriptor.set, undefined);
assert(!descriptor.enumerable && descriptor.configurable);
