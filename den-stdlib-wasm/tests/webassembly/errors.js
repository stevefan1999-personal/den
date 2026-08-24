const { assertEquals } = await import("den:assert");
const caught = async (thunk) => {
  try {
    await thunk();
    return "nothing thrown";
  } catch (error) {
    return `${error.name}/${error instanceof Error}`;
  }
};
const tooSmall = wat2wasm('(module (import "env" "mem" (memory 2)))');
let reentrant = "not reached";
const { instance } = await WebAssembly.instantiate(WASM, {
  env: {
    reenter: () => {
      reentrant = "nothing thrown";
      try {
        instance.exports.run();
      } catch (error) {
        reentrant = `${error.name}/${error instanceof Error}`;
      }
    },
  },
});
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
instance.exports.run();
assertEquals(reentrant, "RuntimeError/true");
