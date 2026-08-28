import { assert, assertThrows } from "den:assert";
import { loadQuickJS } from "./load.js";

const { vm } = await loadQuickJS({
  memoryLimit: 1 * 1024 * 1024,
});

const oom = assertThrows(() => {
  vm.evalCode("const xs = []; for (;;) xs.push(new Array(1e5).fill('x'))");
});
assert(
  String(oom.message).toLowerCase().includes("memory"),
  oom.message,
);
oom.dispose?.();

const usage = vm.getMemoryUsage();
assert(typeof usage.memoryUsedSize === "number");
assert(typeof usage.objCount === "number");
vm.runGC();
vm.dispose();
