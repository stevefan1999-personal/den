// The same-thread branch of the trampoline: a callback C stored earlier and
// fires from the realm's own thread, inside a synchronous call.
//
// §4.7 says this branch is rarer than "qsort, visitor APIs" suggests, and this
// is the shape that reaches it: the callback was handed over through a
// `nonblocking` symbol, C kept it, and a later *synchronous* symbol runs it on
// the thread that is already holding the runtime lock. No mailbox, no worker.
import { assertEquals } from "den:assert";
import { callback, grant, open } from "den:ffi";
import { library } from "./probe.js";

const probe = open(library, {
  store_callback: {
    params: [{ callback: { params: ["i32"], result: "i32" } }],
    result: "void",
    nonblocking: true,
  },
  fire_stored: { params: ["i32"], result: "i32" },
}, grant());

using double = callback({ params: ["i32"], result: "i32" }, (n) => n * 2);
await probe.store_callback(double);

// Re-entrant: JS → C → JS, all on one thread, all under one runtime lock.
assertEquals(probe.fire_stored(21), 42);

probe[Symbol.dispose]();
