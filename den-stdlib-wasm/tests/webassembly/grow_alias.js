import { assertEquals } from "den:assert";
import { wat2wasm } from "den:wasm";

const GROWS = wat2wasm(`
    (module
      (memory (export "mem") 1)
      (func (export "boom") unreachable)
      (func (export "grow") (drop (memory.grow (i32.const 1)))))
`);
const REEXPORTS = wat2wasm(`
    (module
      (import "env" "mem" (memory 1))
      (export "mem" (memory 0)))
`);

const { mem, grow, boom } = (await WebAssembly.instantiate(GROWS)).instance.exports;
const stale = mem.buffer;
const view = new Uint8Array(stale);
grow();
assertEquals(stale.byteLength, 0);
assertEquals(view.length, 0);
assertEquals(mem.buffer !== stale, true);
assertEquals(mem.buffer.byteLength, 131072);

let thrown = "nothing thrown";
try {
  boom();
} catch (error) {
  thrown = `${error.name}/${error instanceof Error}`;
}
assertEquals(thrown, "RuntimeError/true");

const afterModule = new WebAssembly.Instance(new WebAssembly.Module(GROWS)).exports.boom;
let afterTyped = "nothing thrown";
try {
  afterModule();
} catch (error) {
  afterTyped = `${error.name}/${error instanceof Error}`;
}
assertEquals(afterTyped, "RuntimeError/true");

const imported = new WebAssembly.Memory({ initial: 1, maximum: 4 });
const reexported = (await WebAssembly.instantiate(REEXPORTS, { env: { mem: imported } }))
  .instance.exports.mem;
const otherStale = reexported.buffer;
imported.grow(1);
assertEquals(reexported !== imported, true);
assertEquals(otherStale.byteLength, 0);
assertEquals(reexported.buffer.byteLength, 131072);
assertEquals(reexported.buffer === imported.buffer, true);

const memory = new WebAssembly.Memory({ initial: 1, maximum: 4 });
const module = new WebAssembly.Module(REEXPORTS);
let outcome = "not reached";
try {
  memory.grow({
    valueOf: () => {
      new WebAssembly.Instance(module, { env: { mem: memory } });
      return 1;
    },
  });
  outcome = `grew/${memory.buffer.byteLength}`;
} catch (error) {
  outcome = `${error.name}: ${error.message}`;
}
assertEquals(outcome, "grew/131072");
