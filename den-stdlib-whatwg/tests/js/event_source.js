import { assert, assertEquals } from "den:assert";

const url = process.env.DEN_TEST_URL;
let relativeThrew = false;
try {
  new EventSource("/relative");
} catch (error) {
  relativeThrew = error.name === "SyntaxError";
}
const es = new EventSource(url);
const custom = new Promise((resolve) => es.addEventListener("custom", (e) => resolve(e)));
const message = new Promise((resolve) => {
  es.onmessage = (e) => resolve(e);
});
await new Promise((resolve, reject) => {
  es.onopen = () => resolve();
  es.onerror = () => reject(new Error("eventsource error " + es.readyState));
});
const first = await custom;
const second = await message;
es.close();
assert(relativeThrew);
assertEquals(es.readyState, EventSource.CLOSED);
assertEquals(first.data, "a");
assert(first instanceof MessageEvent);
assertEquals(second.data, "b");
assert(second.origin.startsWith("http://127.0.0.1"));
