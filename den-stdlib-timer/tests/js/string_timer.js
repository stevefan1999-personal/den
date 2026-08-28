import { assertEquals, assertThrows } from "den:assert";

assertThrows(() => setTimeout("globalThis.ran = true", 1), TypeError);
assertThrows(() => setInterval(null, 1), TypeError);

const received = await new Promise((resolve) => {
  setTimeout((...values) => resolve(values), 1, "native", 42);
});
assertEquals(received, ["native", 42]);
