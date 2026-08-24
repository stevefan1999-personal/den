import { wat2wasm } from "den:wasm";

export const WASM = wat2wasm(`
    (module
      (@custom "greeting" "hello")
      (func (export "add") (param i32 i32) (result i32) local.get 0 local.get 1 i32.add)
      (func (export "boom") unreachable))
`);
