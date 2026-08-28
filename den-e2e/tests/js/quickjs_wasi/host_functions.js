import { assertEquals } from "den:assert";
import { loadQuickJS } from "./load.js";

const { vm } = await loadQuickJS();

const seen = [];
const log = vm.newFunction("log", function (...args) {
  seen.push(args.map((value) => vm.dump(value)));
  return vm.undefined;
});
vm.global.setProp("log", log);
log.dispose();

const sum = vm.newFunction("sum", function (left, right) {
  return vm.newNumber(left.toNumber() + right.toNumber());
});
vm.global.setProp("sum", sum);
sum.dispose();

vm.evalCode('log("sum", sum(40, 2))').dispose();
assertEquals(seen, [["sum", 42]]);

const nameOf = vm.newFunction("nameOf", function () {
  return this.getProp("label");
});
const target = vm.newObject();
const label = vm.newString("den");
target.setProp("label", label);
target.setProp("nameOf", nameOf);
vm.global.setProp("target", target);
label.dispose();
nameOf.dispose();
target.dispose();

const named = vm.evalCode("target.nameOf()");
assertEquals(named.toString(), "den");
named.dispose();

const nested = vm.newFunction("nested", function () {
  return vm.evalCode("40 + 2");
});
vm.global.setProp("nested", nested);
nested.dispose();
const again = vm.evalCode("nested()");
assertEquals(again.toNumber(), 42);
again.dispose();

vm.dispose();
