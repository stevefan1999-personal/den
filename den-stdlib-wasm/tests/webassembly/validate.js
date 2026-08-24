const { assertEquals } = await import("den:assert");
assertEquals(WebAssembly.validate(WASM), true);
assertEquals(WebAssembly.validate(new Uint8Array([1, 2, 3])), false);
