const { assert, assertEquals, assertThrows } = await import("den:assert");
const { echo, beyond } = (await WebAssembly.instantiate(WASM)).instance.exports;
assertEquals(typeof beyond(), "bigint");
assertEquals(beyond(), 9007199254740993n);
assertEquals(echo(-9007199254740993n), -9007199254740993n);
assertThrows(() => echo(1), TypeError);
