import { assertEquals } from "den:assert";
import { WASM } from "./hello.js";

const response = (contentType) => ({
  ok: true,
  status: 200,
  headers: { get: (name) => name.toLowerCase() === "content-type" ? contentType : null },
  arrayBuffer: async () => WASM.buffer,
});
const { instance } = await WebAssembly.instantiateStreaming(
  Promise.resolve(response("application/wasm")),
);
let rejected = "no rejection";
try {
  await WebAssembly.compileStreaming(response("text/html"));
} catch (error) {
  rejected = error.name;
}
assertEquals(instance.exports.add(1, 2), 3);
assertEquals(rejected, "TypeError");
