import { assertEquals } from "den:assert";
import { WASM } from "./hello.js";

const module = new WebAssembly.Module(WASM);
const instance = new WebAssembly.Instance(module);
assertEquals(module instanceof WebAssembly.Module, true);
assertEquals(instance instanceof WebAssembly.Instance, true);
assertEquals(instance.exports.add(2, 2), 4);

const attempt = (source) => {
  try {
    new WebAssembly.Module(source);
    return "compiled";
  } catch (error) {
    return error.name;
  }
};
assertEquals(
  [
    attempt(WASM),
    attempt(WASM.buffer),
    attempt(new DataView(WASM.buffer)),
    attempt({ buffer: WASM.buffer, byteOffset: 0, byteLength: WASM.byteLength }),
    attempt("not a buffer"),
  ].join(","),
  "compiled,compiled,compiled,TypeError,TypeError",
);
