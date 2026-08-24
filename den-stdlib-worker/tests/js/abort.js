import { assert, assertEquals } from "den:assert";

const listenerLog = [];
const target = new EventTarget();
const dead = new AbortController();
dead.abort();
target.addEventListener("ping", () => listenerLog.push("never"), { signal: dead.signal });
const live = new AbortController();
target.addEventListener("ping", () => listenerLog.push("aborted-later"), { signal: live.signal });
target.addEventListener("ping", () => listenerLog.push("unsignalled"));
target.dispatchEvent(new Event("ping"));
live.abort();
target.dispatchEvent(new Event("ping"));
assertEquals(listenerLog.join(","), "aborted-later,unsignalled,unsignalled");

const fresh = new AbortController();
assert(fresh.signal instanceof AbortSignal);
assert(fresh.signal instanceof EventTarget);
assertEquals(fresh.signal.aborted, false);
assertEquals(fresh.signal.reason, undefined);

const defaulted = new AbortController();
defaulted.abort();
assertEquals(defaulted.signal.aborted, true);
assert(defaulted.signal.reason instanceof DOMException);
assertEquals(defaulted.signal.reason.name, "AbortError");

const custom = new AbortController();
const customReason = new Error("custom reason");
custom.abort(customReason);
assertEquals(custom.signal.aborted, true);
assertEquals(custom.signal.reason, customReason);

const once = new AbortController();
const first = new Error("first");
once.abort(first);
once.abort(new Error("second"));
assertEquals(once.signal.reason, first);

const listened = new AbortController();
let listenerCount = 0;
listened.signal.addEventListener("abort", () => { listenerCount++; });
let onabortCalled = false;
listened.signal.onabort = () => { onabortCalled = true; };
listened.abort();
listened.abort();
assertEquals(listenerCount, 1);
assert(onabortCalled);

const throwing = new AbortController();
let threwWhileLive = false;
try {
  throwing.signal.throwIfAborted();
} catch {
  threwWhileLive = true;
}
assert(!threwWhileLive);
const thrownReason = new Error("aborted!");
throwing.abort(thrownReason);
let threw = false;
try {
  throwing.signal.throwIfAborted();
} catch (error) {
  threw = error === thrownReason;
}
assert(threw);

const preAborted = AbortSignal.abort();
assertEquals(preAborted.aborted, true);
assert(preAborted.reason instanceof DOMException);
assertEquals(preAborted.reason.name, "AbortError");
const staticReason = new Error("custom");
assertEquals(AbortSignal.abort(staticReason).reason, staticReason);

const firstSource = new AbortController();
const secondSource = new AbortController();
const combined = AbortSignal.any([firstSource.signal, secondSource.signal]);
assertEquals(combined.aborted, false);
let anyFired = false;
combined.addEventListener("abort", () => { anyFired = true; });
firstSource.abort(new Error("c1"));
assertEquals(combined.aborted, true);
assertEquals(combined.reason.message, "c1");
assert(anyFired);

let anyCount = 0;
const a = new AbortController();
const b = new AbortController();
const onceCombined = AbortSignal.any([a.signal, b.signal]);
onceCombined.addEventListener("abort", () => { anyCount++; });
a.abort();
b.abort();
assertEquals(anyCount, 1);

const already = new AbortController();
already.abort(new Error("already"));
const fromAborted = AbortSignal.any([already.signal]);
assertEquals(fromAborted.aborted, true);
assertEquals(fromAborted.reason.message, "already");

const timed = AbortSignal.timeout(50);
assertEquals(timed.aborted, false);
await new Promise((resolve) => setTimeout(resolve, 80));
assertEquals(timed.aborted, true);
assert(timed.reason instanceof DOMException);
assertEquals(timed.reason.name, "TimeoutError");

assertEquals(
  Object.prototype.toString.call(new AbortController()),
  "[object AbortController]",
);
assertEquals(
  Object.prototype.toString.call(new AbortController().signal),
  "[object AbortSignal]",
);
