const { assert, assertEquals } = await import("den:assert");
const { exports } = (await WebAssembly.instantiate(WASM)).instance;
assert(Object.isFrozen(exports));
assertEquals(Object.getPrototypeOf(exports), null);
assert((await WebAssembly.instantiate(WASM)).instance.exports !== exports);
