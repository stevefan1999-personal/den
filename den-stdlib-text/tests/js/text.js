const { assertEquals, assert } = await import("den:assert");
const encoded = new TextEncoder().encode("héllo €");
const decoded = new TextDecoder().decode(encoded);
assert(encoded instanceof Uint8Array);
assertEquals(encoded.length, 10);
assertEquals(decoded, "héllo €");
