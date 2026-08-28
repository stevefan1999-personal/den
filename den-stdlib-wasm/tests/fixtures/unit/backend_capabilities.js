const observations = [];
const attempt = (f) => {
  try {
    return String(f());
  } catch (error) {
    return error.name;
  }
};
const observe = (what, f) => observations.push(`${what}: ${attempt(f)}`);
const validates = (wat) => WebAssembly.validate(globalThis.wat2wasm(wat));
const run = (wat, name, ...args) =>
  new WebAssembly.Instance(new WebAssembly.Module(globalThis.wat2wasm(wat)))
    .exports[name](...args);

// Shared memory: refused at both the JS API and the module level.
observe("a shared memory", () => new WebAssembly.Memory({ initial: 1, maximum: 1, shared: true }));
observe("a shared memory without a maximum", () => new WebAssembly.Memory({ initial: 1, shared: true }));
observe("a module with a shared memory", () => validates(`(module (memory 1 1 shared))`));
observe("a module using atomics", () =>
  validates(`(module (memory 1 1 shared) (func (result i32) i32.const 0 i32.atomic.load))`));

// Value types with no JS representation, rejected by the shared
// descriptor plumbing rather than by the engine.
observe("an anyref global", () => new WebAssembly.Global({ value: "anyref" }, null));
observe("a v128 global", () => new WebAssembly.Global({ value: "v128" }, 0));

observe("a module using anyref", () => validates(`(module (func (result anyref) ref.null any))`));
observe("a module using v128", () => validates(`(module (func (result v128) v128.const i32x4 0 0 0 0))`));
observe("a module with a tag", () => validates(`(module (tag (param i32)))`));
observe("a tag", () => new WebAssembly.Tag({ parameters: ["i32"] }) instanceof WebAssembly.Tag);

observe("a module using tail calls", () => validates(`(module (func $f) (func (return_call $f)))`));
observe("a module using extended const", () =>
  validates(`(module (global i32 (i32.add (i32.const 1) (i32.const 2))))`));
observe("a module with two memories", () => validates(`(module (memory 1) (memory 1))`));
observe("a module using memory64", () => validates(`(module (memory i64 1))`));
observe("a module using custom page sizes", () => validates(`(module (memory 1 (pagesize 1)))`));
observe("a module using bulk memory", () =>
  validates(`(module (memory 1) (func (memory.fill (i32.const 0) (i32.const 0) (i32.const 1))))`));

observe("an exported function", () =>
  run(`(module (func (export "add") (param i32 i32) (result i32)
         local.get 0 local.get 1 i32.add))`, "add", 1, 2));
observe("a trapping export", () => run(`(module (func (export "boom") unreachable))`, "boom"));

observations
