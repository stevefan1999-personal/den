import { assertEquals } from "den:assert";
import { loadQuickJS } from "./load.js";

const { CompileFlags, vm } = await loadQuickJS();

const bytecode = vm.compile("40 + 2", "add.js", 0, CompileFlags.STRIP_SOURCE);
const compiled = vm.evalBytecode(bytecode);
assertEquals(compiled.toNumber(), 42);
compiled.dispose();

vm.dispose();
