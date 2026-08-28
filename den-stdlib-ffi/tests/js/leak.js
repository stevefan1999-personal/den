// §0 fact 11: a callback that is still registered when the realm goes away.
// The registry lives in context userdata, which rquickjs clears *before*
// `JS_FreeRuntime`, so this is an ordinary shutdown. Held in a module-level
// binding on purpose — the point is that nothing disposes it.
import { callback } from "den:ffi";

export const live = callback({ params: ["i32"], result: "i32" }, (n) => n);
