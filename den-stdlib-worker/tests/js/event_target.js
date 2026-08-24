import { assertEquals } from "den:assert";

const seen = [];
addEventListener("ping", (event) => seen.push(event.type));
const dispatched = dispatchEvent(new Event("ping"));
assertEquals(typeof addEventListener, "function");
assertEquals(typeof onunhandledrejection, "object");
assertEquals(dispatched, true);
assertEquals(seen.join(","), "ping");
