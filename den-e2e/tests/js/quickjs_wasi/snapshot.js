import { assertEquals } from "den:assert";
import { loadQuickJS } from "./load.js";

const { QuickJS, wasm, vm } = await loadQuickJS();

const log = vm.newFunction("log", function (value) {
  return vm.newNumber(value.toNumber() + 1);
});
vm.global.setProp("log", log);
log.dispose();
vm.evalCode("var n = 40").dispose();

const snap = vm.snapshot();
const bytes = QuickJS.serializeSnapshot(snap);
const restored = await QuickJS.restore(QuickJS.deserializeSnapshot(bytes), {
  wasm,
});

const next = restored.evalCode("n + 2");
assertEquals(next.toNumber(), 42);
next.dispose();

restored.registerHostCallback("log", function (value) {
  return restored.newNumber(value.toNumber() + 2);
});
const rebound = restored.evalCode("log(n)");
assertEquals(rebound.toNumber(), 42);
rebound.dispose();

restored.dispose();
vm.dispose();
