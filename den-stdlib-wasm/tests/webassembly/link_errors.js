import { assertEquals } from "den:assert";
import { wat2wasm } from "den:wasm";

const CALLS_IMPORT = wat2wasm(`
    (module
      (import "env" "log" (func $log (param i32 i32) (result i32)))
      (func (export "run") (param i32 i32) (result i32)
        local.get 0 local.get 1 call $log))
`);
const NEEDS_TWO_PAGES = wat2wasm(`(module (import "env" "mem" (memory 2)))`);
const WANTS_WASI = wat2wasm(`
    (module (import "wasi_snapshot_preview1" "proc_exit" (func (param i32))))
`);
const WANTS_I64 = wat2wasm(`(module (import "env" "g" (global i64)))`);
const WANTS_I32 = wat2wasm(`(module (import "env" "g" (global i32)))`);

const nameOf = async (thunk) => {
  try {
    await thunk();
    return "no rejection";
  } catch (error) {
    return error.name;
  }
};

assertEquals(await nameOf(() => WebAssembly.instantiate(CALLS_IMPORT, {})), "TypeError");
assertEquals(
  await nameOf(() => WebAssembly.instantiate(NEEDS_TWO_PAGES, {
    env: { mem: new WebAssembly.Memory({ initial: 1 }) },
  })),
  "LinkError",
);

const wasiModule = new WebAssembly.Module(WANTS_WASI);
const attempt = (importObject) => {
  try {
    new WebAssembly.Instance(wasiModule, importObject);
    return "instantiated";
  } catch (error) {
    return error.name;
  }
};
assertEquals([attempt(undefined), attempt(5)].join(","), "TypeError,TypeError");

const wrongFlavour = (bytes, value) => {
  try {
    new WebAssembly.Instance(new WebAssembly.Module(bytes), { env: { g: value } });
    return "no rejection";
  } catch (error) {
    return error.name;
  }
};
assertEquals(wrongFlavour(WANTS_I64, 1), "LinkError");
assertEquals(wrongFlavour(WANTS_I32, 1n), "LinkError");

const mem = new WebAssembly.Memory({ initial: 1, maximum: 4 });
mem.grow(1);
let grown = "not reached";
try {
  await WebAssembly.instantiate(NEEDS_TWO_PAGES, { env: { mem } });
  grown = "instantiated";
} catch (error) {
  grown = error.name;
}
assertEquals(grown, "instantiated");
