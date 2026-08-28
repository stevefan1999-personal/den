import { assert, assertThrows } from "den:assert";
import { loadQuickJS } from "./load.js";

let deadline = Number.POSITIVE_INFINITY;
const { vm } = await loadQuickJS({
  interruptHandler() {
    return Date.now() > deadline;
  },
});
deadline = Date.now() + 250;

const interrupted = assertThrows(() => {
  vm.evalCode("for (;;) {}");
});
assert(
  String(interrupted.message).toLowerCase().includes("interrupt"),
  interrupted.message,
);
interrupted.dispose?.();
vm.dispose();
