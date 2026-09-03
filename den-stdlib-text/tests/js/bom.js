import { assertEquals, assertThrows } from "den:assert";

const bom = new Uint8Array([0xef, 0xbb, 0xbf, 0x61]);
assertEquals(new TextDecoder("utf-8").decode(bom), "a");
assertEquals(new TextDecoder("utf-8", { ignoreBOM: true }).decode(bom), "\uFEFFa");
assertEquals(new TextDecoder("utf-8", { ignoreBOM: true }).ignoreBOM, true);

// BOM sniffing switches the encoding, whatever the label said, unless
// ignoreBOM keeps the bytes as data; a truncated BOM is plain (malformed) data.
const utf16le = new Uint8Array([0xff, 0xfe, 0x61, 0x00]);
assertEquals(new TextDecoder("utf-8").decode(utf16le), "a");
assertEquals(new TextDecoder("utf-16be").decode(utf16le), "a");
assertEquals(new TextDecoder("utf-8", { fatal: true }).decode(utf16le), "a");
assertEquals(new TextDecoder("utf-8", { ignoreBOM: true }).decode(utf16le), "\uFFFD\uFFFDa\u0000");
assertEquals(new TextDecoder("utf-8").decode(new Uint8Array([0xef, 0xbb])), "\uFFFD");
assertThrows(
  () => new TextDecoder("utf-8", { fatal: true }).decode(new Uint8Array([0xff, 0xfe, 0x61])),
  TypeError,
  "invalid decoding",
);
assertEquals(new TextDecoder("utf-8").decode(new Uint8Array([0xef, 0xbb, 0xbf])), "");
assertEquals(new TextDecoder("utf-8").decode(new Uint8Array([])), "");
assertEquals(new TextDecoder("windows-1252").decode(new Uint8Array([0x80, 0xff])), "\u20AC\u00FF");
assertEquals(new TextDecoder("utf-16le").decode(new Uint8Array([0x61])), "\uFFFD");
