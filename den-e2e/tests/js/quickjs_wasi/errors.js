import { assert, assertEquals, assertThrows } from "den:assert";
import { loadQuickJS } from "./load.js";

const { JSException, vm } = await loadQuickJS();

const thrown = assertThrows(() => {
  vm.evalCode("throw new TypeError('bad input')");
}, JSException);
assertEquals(thrown.name, "TypeError");
assertEquals(thrown.message, "bad input");
assert(typeof thrown.stack === "string");
thrown.dispose();

const syntax = assertThrows(() => {
  vm.evalCode("const = 1");
}, JSException);
assertEquals(syntax.name, "SyntaxError");
syntax.dispose();

const named = assertThrows(() => {
  vm.evalCode("throw new Error('here')", "guest.js");
}, JSException);
const guestStack = named.handle.getProp("stack");
assert(
  guestStack.toString().includes("guest.js"),
  guestStack.toString(),
);
guestStack.dispose();
named.dispose();

const hosted = vm.newError("from host");
vm.global.setProp("hostedError", hosted);
hosted.dispose();
const fromHost = assertThrows(() => {
  vm.evalCode("throw hostedError");
}, JSException);
assertEquals(fromHost.message, "from host");
fromHost.dispose();

vm.dispose();
