import { assertEquals } from "den:assert";

const promise = Promise.resolve();
const reason = new Error("boom");
const event = new PromiseRejectionEvent("unhandledrejection", {
  promise, reason, cancelable: true,
});
let arity = "accepted";
try {
  new PromiseRejectionEvent("unhandledrejection");
} catch (error) {
  arity = error.constructor.name;
}
const target = new EventTarget();
target.addEventListener("unhandledrejection", (seen) => seen.preventDefault());
assertEquals(event.type, "unhandledrejection");
assertEquals(event.promise === promise, true);
assertEquals(event.reason === reason, true);
assertEquals(event.cancelable, true);
assertEquals(arity, "TypeError");
assertEquals(target.dispatchEvent(event), false);
