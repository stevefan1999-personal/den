const bytes = globalThis.wat2wasm(`(module
  (func (export "add") (param i32 i32) (result i32) local.get 0 local.get 1 i32.add))`);
const { add } = new WebAssembly.Instance(new WebAssembly.Module(bytes)).exports;
const table = new WebAssembly.Table({ element: "anyfunc", initial: 1 });
table.set(0, add);
[table.get(0) === add, table.get(0) === table.get(0), table.get(0)(20, 22),
 add.name, add.length].join(",")
