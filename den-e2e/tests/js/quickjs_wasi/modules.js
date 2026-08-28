import { assert } from "den:assert";
import { loadQuickJS, settle } from "./load.js";

const files = new Map([
  ["math.js", "export const add = (a, b) => a + b;"],
  ["main.js", "import { add } from 'math.js'; export default add(40, 2);"],
]);
const requested = [];
const { EvalFlags, vm } = await loadQuickJS({
  moduleLoader: {
    load(specifier) {
      requested.push(specifier);
      const source = files.get(specifier);
      if (!source) throw new Error(`unknown module: ${specifier}`);
      return source;
    },
  },
});

const modulePromise = vm.evalCode(
  files.get("main.js"),
  "main.js",
  EvalFlags.TYPE_MODULE,
);
assert(modulePromise.isPromise);
const moduleResult = await settle(vm, modulePromise);
assert("value" in moduleResult);
assert(requested.includes("math.js"));
moduleResult.value.dispose();
modulePromise.dispose();
vm.dispose();
