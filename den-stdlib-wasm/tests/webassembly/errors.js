import { assertEquals } from "den:assert";

const WASM = globalThis.wat2wasm(`
    (module
      (func (export "boom") unreachable))
`);
const caught = async (thunk) => {
  try {
    await thunk();
    return "nothing thrown";
  } catch (error) {
    return `${error.name}/${error instanceof Error}`;
  }
};
const tooSmall = globalThis.wat2wasm('(module (import "env" "mem" (memory 2)))');
const { instance } = await WebAssembly.instantiate(WASM);
assertEquals(
  await caught(() => WebAssembly.compile(new Uint8Array([0, 97, 115, 109, 9, 9, 9, 9]))),
  "CompileError/true",
);
assertEquals(
  await caught(() => WebAssembly.instantiate(tooSmall, {
    env: { mem: new WebAssembly.Memory({ initial: 1 }) },
  })),
  "LinkError/true",
);
assertEquals(
  await caught(() => instance.exports.boom()),
  "RuntimeError/true",
);
