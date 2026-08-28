export const WASM = globalThis.wat2wasm(`
    (module
      (@custom "greeting" "hello")
      (func (export "add") (param i32 i32) (result i32) local.get 0 local.get 1 i32.add)
      (func (export "boom") unreachable))
`);
