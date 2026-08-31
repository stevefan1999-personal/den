import { assertEquals, assert } from "den:assert";
const encoded = new TextEncoder().encode("héllo €");
const decoded = new TextDecoder().decode(encoded);
assert(encoded instanceof Uint8Array);
assertEquals(encoded.length, 10);
assertEquals(decoded, "héllo €");

const isView = ArrayBuffer.isView;
ArrayBuffer.isView = () => false;
const view = new Uint8Array([65]);
Object.defineProperty(view, "buffer", { value: new ArrayBuffer(0) });
assertEquals(new TextDecoder().decode(view), "A");
ArrayBuffer.isView = isView;
