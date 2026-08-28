import { assert, assertEquals } from "den:assert";

const buffer = new ArrayBuffer(16);
const prefix = new Uint8Array(buffer, 0, 8);
const view = new Uint8Array(buffer, 8, 4);
const suffix = new Uint8Array(buffer, 12, 4);
prefix.fill(0x11);
suffix.fill(0x22);

const returned = crypto.getRandomValues(view);
assert(returned === view);
assert(prefix.every((byte) => byte === 0x11));
assert(suffix.every((byte) => byte === 0x22));
assert(view.some((byte) => byte !== 0));
assertEquals(view.length, 4);
