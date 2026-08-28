import { assertEquals, assertThrows } from "den:assert";

const replaced = new TextDecoder("utf-8").decode(new Uint8Array([0xff]));
assertEquals(replaced.length > 0, true);

const fatal = new TextDecoder("utf-8", { fatal: true });
assertEquals(fatal.fatal, true);
assertThrows(
  () => fatal.decode(new Uint8Array([0xff])),
  TypeError,
  "invalid decoding",
);
