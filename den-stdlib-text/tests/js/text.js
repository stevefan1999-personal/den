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

const source = new ArrayBuffer(4);
new Uint8Array(source).set([88, 65, 66, 89]);
assertEquals(new TextDecoder().decode(new Uint16Array(source)), "XABY");
const dataView = new DataView(source, 1, 2);
Object.defineProperties(dataView, {
  buffer: { value: new ArrayBuffer(0) },
  byteOffset: { value: 0 },
  byteLength: { value: 0 },
});
const DataViewConstructor = DataView;
globalThis.DataView = function ReplacedDataView() {};
assertEquals(new TextDecoder().decode(dataView), "AB");
globalThis.DataView = DataViewConstructor;
ArrayBuffer.isView = isView;
