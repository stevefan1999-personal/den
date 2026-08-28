import { assertEquals } from "den:assert";
import { loadQuickJS } from "./load.js";

const { vm } = await loadQuickJS();

const number = vm.evalCode("40 + 2");
assertEquals(number.toNumber(), 42);
number.dispose();

const text = vm.evalCode('"hello from QuickJS " + (40 + 2)');
assertEquals(text.toString(), "hello from QuickJS 42");
text.dispose();

vm.evalCode("var acc = 0; function add(n) { acc += n; return acc; }").dispose();
const a = vm.evalCode("add(40)");
const b = vm.evalCode("add(2)");
assertEquals(a.toNumber(), 40);
assertEquals(b.toNumber(), 42);
a.dispose();
b.dispose();

vm.dispose();
