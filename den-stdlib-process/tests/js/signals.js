import { assert } from "den:assert";
const listener = () => {};
process.addSignalListener("SIGTERM", listener);
process.removeSignalListener("SIGTERM", listener);
assert(true);
