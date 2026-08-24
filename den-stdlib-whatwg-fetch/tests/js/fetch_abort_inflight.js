import { assertEquals } from "den:assert";
const listeners = [];
const signal = {
  aborted: false,
  addEventListener(type, fn) {
    if (type === "abort") listeners.push(fn);
  },
  abort() {
    this.aborted = true;
    for (const fn of listeners) fn();
  },
};
const pending = fetch(process.env.DEN_TEST_URL, { signal });
await Promise.resolve();
signal.abort();
let name = "not-aborted";
try {
  await pending;
} catch (error) {
  name = error.name;
}
assertEquals(name, "AbortError");
