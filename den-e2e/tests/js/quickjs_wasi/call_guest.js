import { assertEquals } from "den:assert";
import { loadQuickJS } from "./load.js";

const { vm } = await loadQuickJS();

vm.evalCode("function greet(name) { return 'hi ' + name; }").dispose();
const greet = vm.global.getProp("greet");
const arg = vm.newString("QuickJS");
const out = vm.callFunction(greet, vm.undefined, arg);
assertEquals(out.toString(), "hi QuickJS");
out.dispose();
arg.dispose();
greet.dispose();

vm.dispose();
