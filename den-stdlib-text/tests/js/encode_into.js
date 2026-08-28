import { assertEquals } from "den:assert";

const dest = new Uint8Array(4);
const encoder = new TextEncoder();
assertEquals(typeof encoder.encodeInto, "function");
const result = encoder.encodeInto("héllo", dest);
assertEquals(result.written, 4);
assertEquals(result.read, 3);
assertEquals(new TextDecoder().decode(dest.subarray(0, result.written)), "hél");
assertEquals(encoder.encoding, "utf-8");
assertEquals(new TextDecoder().decode(new Uint8Array()), "");
