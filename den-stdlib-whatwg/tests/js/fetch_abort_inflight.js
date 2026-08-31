import { assertEquals } from "den:assert";
import { serve } from "den:http";

let started;
const requestStarted = new Promise((resolve) => (started = resolve));
const server = serve({
  listen: { host: "127.0.0.1", port: 0 },
  fetch() {
    started();
    return new Promise(() => {});
  },
});
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
try {
  const pending = fetch(server.url, { signal });
  await requestStarted;
  signal.abort();
  let name = "not-aborted";
  try {
    await pending;
  } catch (error) {
    name = error.name;
  }
  assertEquals(name, "AbortError");
} finally {
  await server.close({ drainMs: 0 });
}
