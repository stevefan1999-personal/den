import { assertEquals, assertThrows } from "den:assert";

assertThrows(() => new TextDecoder("no-such-encoding"), RangeError, "unknown encoding");
assertEquals(new TextDecoder("utf-16le").encoding, "utf-16le");
