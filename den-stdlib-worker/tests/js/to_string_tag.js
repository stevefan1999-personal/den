import { assertEquals } from "den:assert";

const branded = [
  "AbortController",
  "AbortSignal",
  "Event",
  "CustomEvent",
  "MessageEvent",
  "ErrorEvent",
  "PromiseRejectionEvent",
  "EventTarget",
  "MessagePort",
  "MessageChannel",
  "Worker",
  "BroadcastChannel",
  "NavigatorUAData",
];
const tagOf = (object) => Object.prototype.toString.call(object).slice("[object ".length, -1);
for (const name of branded) {
  assertEquals(tagOf(globalThis[name].prototype), name);
}
assertEquals(tagOf(new Event("x")), "Event");
