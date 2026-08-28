// §4.7's residual case, and the bound that turns it from a hang into a stall.
//
// The callback is registered through one symbol and stored by C. A *different*
// symbol — synchronous, taking no callback at all, so there is nothing at its
// call site for den to refuse — then spawns a thread, fires the stored
// callback there and joins it. The realm is inside that C frame, so it can
// never reach the pump: this is a true cycle, and no marshal-time check can
// see it coming.
//
// The foreign thread's wait is bounded. On expiry it hands C the zero value
// and writes one line to stderr naming the library, the symbol and the
// timeout. C's thread then finishes, the join returns, the synchronous call
// returns, and the realm resumes — which is the whole point. Without the bound
// this script never reaches its last line.
import { assertEquals } from "den:assert";
import { callback, grant, open } from "den:ffi";
import { library } from "./probe.js";

const probe = open(library, {
  store_callback: {
    params: [{ callback: { params: ["i32"], result: "i32" } }],
    result: "void",
    nonblocking: true,
  },
  fire_on_thread_and_join: { params: ["i32"], result: "i32" },
}, grant());

using double = callback({ params: ["i32"], result: "i32" }, (n) => n * 2);
await probe.store_callback(double);

// 0, not 42: the answer C got is a lie, and den said so on stderr. §5.1 item 5
// documents that rather than pretending it is fixed.
assertEquals(probe.fire_on_thread_and_join(21), 0);

probe[Symbol.dispose]();
