const { assertEquals } = await import("den:assert");
let seen = null;
const { instance } = await WebAssembly.instantiate(WASM, {
  env: { log: (left, right) => { seen = [left, right]; return left * right; } },
});
assertEquals(instance.exports.run(6, 7), 42);
assertEquals(seen.join(","), "6,7");
