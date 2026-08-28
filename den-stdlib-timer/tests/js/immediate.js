import { assertEquals } from "den:assert";

const received = await new Promise((resolve) => setImmediate(resolve, "immediate"));
assertEquals(received, "immediate");

let cancelled = true;
const handle = setImmediate(() => {
  cancelled = false;
});
clearImmediate(handle);
await new Promise((resolve) => setTimeout(resolve, 5));
assertEquals(cancelled, true);
