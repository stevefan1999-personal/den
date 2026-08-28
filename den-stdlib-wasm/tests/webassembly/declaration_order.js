import { assertEquals } from "den:assert";

const WASM = globalThis.wat2wasm(`
    (module
      (import "env" "zebra" (func $zebra))
      (import "env" "apple" (global i32))
      (import "env" "middle" (memory 1))
      (import "env" "banana" (table 1 funcref))
      (export "zebra" (func $zebra))
      (export "apple" (memory 0))
      (export "middle" (table 0)))
`);
const values = {
  zebra: () => {},
  apple: 0,
  middle: new WebAssembly.Memory({ initial: 1 }),
  banana: new WebAssembly.Table({ initial: 1, element: "anyfunc" }),
};
const read = [];
const env = {};
for (const name of Object.keys(values)) {
  Object.defineProperty(env, name, {
    get: () => {
      read.push(name);
      return values[name];
    },
    enumerable: true,
  });
}
const { module, instance } = await WebAssembly.instantiate(WASM, { env });
assertEquals(
  [
    read.join(","),
    WebAssembly.Module.imports(module).map((entry) => entry.name).join(","),
    WebAssembly.Module.exports(module).map((entry) => entry.name).join(","),
    Object.keys(instance.exports).join(","),
  ].join(";"),
  "zebra,apple,middle,banana;zebra,apple,middle,banana;zebra,apple,middle;zebra,apple,middle",
);
