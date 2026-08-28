import { assert, assertEquals } from "den:assert";
import { loadQuickJS } from "./load.js";

const { EvalFlags, Intrinsics, JSException, vm } = await loadQuickJS();

assertEquals(EvalFlags.TYPE_GLOBAL, 0);
assertEquals(EvalFlags.TYPE_MODULE, 1);
assertEquals(EvalFlags.STRICT, 8);
assertEquals(EvalFlags.COMPILE_ONLY, 32);
assertEquals(EvalFlags.BACKTRACE_BARRIER, 64);
assertEquals(EvalFlags.ASYNC, 128);
assertEquals(Intrinsics.ALL, 4294967295);
assertEquals(Intrinsics.EVAL, 2);
assertEquals(Intrinsics.PROXY, 16);
assertEquals(typeof JSException, "function");
assertEquals(typeof vm.versions["quickjs-wasi"], "string");
assertEquals(typeof vm.versions.quickjs, "string");
assert(vm.global !== undefined);
assertEquals(vm.dump(vm.undefined), undefined);
assertEquals(vm.dump(vm.null), null);
assertEquals(vm.dump(vm.true), true);
assertEquals(vm.dump(vm.false), false);

vm.dispose();
