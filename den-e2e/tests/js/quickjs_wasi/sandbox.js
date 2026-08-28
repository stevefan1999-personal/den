import { assertThrows } from "den:assert";
import { compileQuickJS } from "./load.js";

const { QuickJS, Intrinsics, wasm } = await compileQuickJS();
const vm = await QuickJS.create({
  wasm,
  memoryLimit: 8 * 1024 * 1024,
  intrinsics: Intrinsics.ALL & ~Intrinsics.EVAL & ~Intrinsics.PROXY,
});

assertThrows(() => {
  // Guest `eval` should be gone: Intrinsics.EVAL was cleared on create.
  vm.evalCode("eval('1')");
});
assertThrows(() => {
  vm.evalCode("new Proxy({}, {})");
});

vm.dispose();
