import { assert, assertEquals } from "den:assert";

// One memory, one table and one global, each at index 0 of its own index
// space, imported and re-exported. The wrapper cache has to hand each one back
// as the very object that was imported: three kinds keyed by one identity
// string must never answer for each other.
const REEXPORTS = globalThis.wat2wasm(`
    (module
      (import "env" "mem" (memory 1))
      (import "env" "tab" (table 1 funcref))
      (import "env" "glob" (global i32))
      (export "mem" (memory 0))
      (export "tab" (table 0))
      (export "glob" (global 0)))
`);
const env = {
  mem: new WebAssembly.Memory({ initial: 1 }),
  tab: new WebAssembly.Table({ element: "anyfunc", initial: 1 }),
  glob: new WebAssembly.Global({ value: "i32" }, 5),
};
const back = (await WebAssembly.instantiate(REEXPORTS, { env })).instance.exports;
assertEquals(back.mem === env.mem, true);
assertEquals(back.tab === env.tab, true);
assertEquals(back.glob === env.glob, true);
assert(back.mem instanceof WebAssembly.Memory);
assert(back.tab instanceof WebAssembly.Table);
assert(back.glob instanceof WebAssembly.Global);

// A second instantiation reuses the same wrappers, and exporting a module's
// own memory/table/global still yields one stable object per handle.
const again = (await WebAssembly.instantiate(REEXPORTS, { env })).instance.exports;
assertEquals(again.mem === env.mem, true);
assertEquals(again.tab === env.tab, true);
assertEquals(again.glob === env.glob, true);

const OWN = globalThis.wat2wasm(`
    (module
      (memory (export "mem") 1)
      (table (export "tab") 1 funcref)
      (global (export "glob") i32 (i32.const 3)))
`);
const own = (await WebAssembly.instantiate(OWN)).instance.exports;
assertEquals(own.mem === own.mem, true);
assertEquals(own.tab === own.tab, true);
assertEquals(own.glob === own.glob, true);
assert(own.mem instanceof WebAssembly.Memory);
assert(own.tab instanceof WebAssembly.Table);
assert(own.glob instanceof WebAssembly.Global);
assertEquals(own.glob.value, 3);
