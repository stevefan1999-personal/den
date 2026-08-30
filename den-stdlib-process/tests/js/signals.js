import { assert } from "den:assert";
const listener = () => {};
process.addSignalListener("SIGINT", listener);
process.removeSignalListener("SIGINT", listener);
assert(true);
