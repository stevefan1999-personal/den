import { assertEquals, assertThrows } from "den:assert";
import { WASM } from "./hello.js";

const module = new WebAssembly.Module(WASM);
const instance = new WebAssembly.Instance(module);
const tag = new WebAssembly.Tag({ parameters: [] });
for (const [Constructor, args, length] of [
  [WebAssembly.Module, [WASM], 1],
  [WebAssembly.Instance, [module], 1],
  [WebAssembly.Memory, [{ initial: 0 }], 1],
  [WebAssembly.Table, [{ element: "externref", initial: 0 }], 1],
  [WebAssembly.Global, [{ value: "i32" }], 1],
  [WebAssembly.Tag, [{ parameters: [] }], 1],
  [WebAssembly.Exception, [tag, []], 2],
]) {
  assertEquals(Constructor.length, length);
  assertEquals(Constructor.prototype.constructor, Constructor);
  assertThrows(() => Constructor(...args), TypeError);
}
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
