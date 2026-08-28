const bytes = globalThis.wat2wasm(`(module
  (func (export "f"))
  (memory (export "m") 1)
  (table (export "t") 1 funcref)
  (global (export "g") i32 (i32.const 0)))`);
const module = new WebAssembly.Module(bytes);
const declared = WebAssembly.Module.exports(module).map((entry) => entry.name).join(",");
const built = Object.keys(new WebAssembly.Instance(module).exports).join(",");
[declared, built].join("|")
