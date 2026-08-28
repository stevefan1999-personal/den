import { assertEquals } from "den:assert";

const bom = new Uint8Array([0xef, 0xbb, 0xbf, 0x61]);
assertEquals(new TextDecoder("utf-8").decode(bom), "a");
assertEquals(new TextDecoder("utf-8", { ignoreBOM: true }).decode(bom), "\uFEFFa");
assertEquals(new TextDecoder("utf-8", { ignoreBOM: true }).ignoreBOM, true);
