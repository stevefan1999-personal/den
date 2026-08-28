// The other half of `armed.js`: disposing the callback ends its pump, and
// `idle()` resolves. This is what `ref()`/`unref()` would otherwise be for.
import { callback } from "den:ffi";

const armed = callback({ params: ["i32"], result: "i32" }, (n) => n);
armed[Symbol.dispose]();
