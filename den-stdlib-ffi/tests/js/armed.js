// An armed callback is a queue the script opened: C may call it at any moment,
// so den stays alive until the script says otherwise (ARCHITECTURE §7.5
// rule 1). Nothing is disposed here, and `tests/ffi.rs` asserts that `idle()`
// therefore never resolves.
import { callback } from "den:ffi";

globalThis.armed = callback({ params: ["i32"], result: "i32" }, (n) => n);
