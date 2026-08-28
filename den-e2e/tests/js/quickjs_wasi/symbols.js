import { assertEquals } from "den:assert";
import { loadQuickJS } from "./load.js";

const { vm } = await loadQuickJS();

const key = vm.newSymbolFor("den:id");
const holder = vm.newObject();
const value = vm.newNumber(7);
vm.setProp(holder, key, value);
vm.global.setProp("holder", holder);
value.dispose();

const read = vm.getProp(holder, key);
assertEquals(read.toNumber(), 7);
read.dispose();
holder.dispose();
key.dispose();

const guest = vm.evalCode("holder[Symbol.for('den:id')]");
assertEquals(guest.toNumber(), 7);
guest.dispose();

vm.dispose();
