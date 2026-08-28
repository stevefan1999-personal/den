const bytes = globalThis.wat2wasm(`(module
  (import "env" "reenter" (func $reenter))
  (func (export "run") (call $reenter))
  (func (export "other") (result i32) (i32.const 1)))`);

let instance;
let result = null;
let caught = null;
instance = new WebAssembly.Instance(new WebAssembly.Module(bytes), {
  env: {
    reenter: () => {
      try {
        result = instance.exports.other();
      } catch (error) {
        caught = error;
      }
    },
  },
});
instance.exports.run();
[result === 1 ? "1" : String(result), caught === null ? "ok" : caught.name]
