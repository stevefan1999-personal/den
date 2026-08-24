import { assertEquals } from "den:assert";
import * as ns from "den:worker";

const documented = [
  "AbortController",
  "AbortSignal",
  "BroadcastChannel",
  "CustomEvent",
  "ErrorEvent",
  "Event",
  "EventTarget",
  "MessageChannel",
  "MessageEvent",
  "MessagePort",
  "NavigatorUAData",
  "PromiseRejectionEvent",
  "Worker",
  "navigator",
  "performance",
  "reportError",
  "structuredClone",
];

const report = documented.map((name) => {
  const exported = ns[name];
  const global = globalThis[name];
  if (exported === undefined) return `${name}: not exported`;
  if (exported !== global) return `${name}: global is not the export`;
  const isObject = name === "performance" || name === "navigator";
  if (isObject) {
    if (typeof exported !== "object" || exported === null) {
      return `${name}: not an object`;
    }
    return `${name}: ok`;
  }
  if (typeof exported !== "function") return `${name}: not a function`;
  const isClass = name !== "structuredClone";
  const hasPrototype = typeof exported.prototype === "object";
  if (isClass !== hasPrototype) return `${name}: wrong kind`;
  if (exported.name !== name) return `${name}: named ${exported.name}`;
  return `${name}: ok`;
});
assertEquals(report.join(","), documented.map((name) => `${name}: ok`).join(","));
