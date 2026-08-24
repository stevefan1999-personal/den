const { assertEquals } = await import("den:assert");
const { boom } = (await WebAssembly.instantiate(WASM)).instance.exports;
let thrown = "nothing thrown";
try {
  boom();
} catch (error) {
  thrown = `${error.name}/${error instanceof Error}`;
}
assertEquals(thrown, "RuntimeError/true");
