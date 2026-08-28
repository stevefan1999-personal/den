import { assert, assertEquals } from "den:assert";
import { loadQuickJS } from "./load.js";

const { vm } = await loadQuickJS();

const bytes = new Uint8Array([1, 2, 3, 4]);
const buffer = vm.newArrayBuffer(bytes);
vm.global.setProp("buffer", buffer);
assert(buffer.isArrayBuffer);
assertEquals(Array.from(new Uint8Array(buffer.toArrayBuffer())), [1, 2, 3, 4]);
buffer.dispose();

const view = vm.evalCode("new Uint8Array(buffer)");
assertEquals(Array.from(view.toUint8Array()), [1, 2, 3, 4]);
view.dispose();

const copy = vm.newUint8Array(new Uint8Array([9, 8, 7]));
vm.global.setProp("copy", copy);
copy.dispose();
const length = vm.evalCode("copy.byteLength");
assertEquals(length.toNumber(), 3);
length.dispose();

vm.dispose();
