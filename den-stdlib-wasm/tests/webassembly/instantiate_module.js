const { assert, assertEquals } = await import("den:assert");
const instance = await WebAssembly.instantiate(await WebAssembly.compile(WASM));
assert(instance instanceof WebAssembly.Instance);
assertEquals(instance.module, undefined);
assertEquals(instance.exports.add(1, 2), 3);
