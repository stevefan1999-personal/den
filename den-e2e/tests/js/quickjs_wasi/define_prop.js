import { assert, assertEquals } from "den:assert";
import { loadQuickJS } from "./load.js";

const { vm } = await loadQuickJS();

const object = vm.newObject();
const hidden = vm.newNumber(1);
object.defineProp("secret", hidden, {
  writable: true,
  enumerable: false,
  configurable: true,
});
hidden.dispose();
vm.global.setProp("object", object);

assertEquals(object.keys().includes("secret"), false);
assert(object.getOwnPropertyNames().includes("secret"));
object.dispose();

const visible = vm.evalCode("Object.keys(object).includes('secret')");
assertEquals(vm.dump(visible), false);
visible.dispose();

const value = vm.evalCode("object.secret");
assertEquals(value.toNumber(), 1);
value.dispose();

vm.dispose();
