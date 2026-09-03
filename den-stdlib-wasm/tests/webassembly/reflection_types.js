import { assert, assertEquals } from "den:assert";

// The js-types reflection dictionaries, key order included.
assertEquals(
  JSON.stringify(new WebAssembly.Global({ value: "i64", mutable: true }, 1n).type()),
  '{"mutable":true,"value":"i64"}',
);
assertEquals(
  JSON.stringify(new WebAssembly.Table({ element: "funcref", initial: 1, maximum: 2 }).type()),
  '{"element":"funcref","minimum":1,"maximum":2}',
);
assertEquals(
  JSON.stringify(new WebAssembly.Table({ element: "externref", initial: 0 }).type()),
  '{"element":"externref","minimum":0}',
);
assertEquals(
  JSON.stringify(new WebAssembly.Memory({ initial: 1, maximum: 3 }).type()),
  '{"minimum":1,"shared":false,"maximum":3}',
);
assertEquals(
  JSON.stringify(new WebAssembly.Memory({ initial: 1 }).type()),
  '{"minimum":1,"shared":false}',
);
assertEquals(
  JSON.stringify(new WebAssembly.Tag({ parameters: ["i32", "f64"] }).type()),
  '{"parameters":["i32","f64"]}',
);

// instantiate(bytes) resolves with `{ module, instance }` in that order.
const EMPTY_MODULE = new Uint8Array([0, 0x61, 0x73, 0x6d, 1, 0, 0, 0]);
const pair = await WebAssembly.instantiate(EMPTY_MODULE);
assertEquals(Object.keys(pair).join(), "module,instance");
assert(pair.module instanceof WebAssembly.Module);
assert(pair.instance instanceof WebAssembly.Instance);
