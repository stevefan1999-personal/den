export const WASM = globalThis.wat2wasm(`
    (module
      (@custom "hello" "world")
      (func (export "nothing"))
      (func (export "one") (result i32) i32.const 7)
      (func (export "pair") (result i32 i32) i32.const 1 i32.const 2)
      (func (export "add") (param i32 i32) (result i32) local.get 0 local.get 1 i32.add))
`);
