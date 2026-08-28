import { assert, assertEquals } from "den:assert";
import { loadQuickJS, settle } from "./load.js";

const { EvalFlags, vm } = await loadQuickJS();

const promise = vm.evalCode(`
  Promise.resolve(21).then((n) => n * 2)
`);
const settled = await settle(vm, promise);
assert("value" in settled);
assertEquals(settled.value.toNumber(), 42);
settled.value.dispose();
promise.dispose();

const pending = vm.evalCode(
  "await Promise.resolve('done')",
  "<eval>",
  EvalFlags.ASYNC,
);
const done = await settle(vm, pending);
assertEquals(vm.dump(done.value), { value: "done" });
done.value.dispose();
pending.dispose();

const deferred = vm.newPromise();
const seed = vm.newNumber(40);
deferred.resolve(seed);
seed.dispose();
vm.global.setProp("hosted", deferred.handle);
deferred.handle.dispose();
vm.evalCode("hosted.then((n) => { globalThis.got = n + 2 })").dispose();
vm.executePendingJobs();
const got = vm.evalCode("got");
assertEquals(got.toNumber(), 42);
got.dispose();

vm.dispose();
