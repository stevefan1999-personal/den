// SIGBREAK is a Windows name: unix has no number for it and no forwarder, so
// `add` must refuse it outright. Accepting it would register a listener that
// can never fire and that `removeSignalListener` would then throw on.
import { assertThrows } from "den:assert";

const listener = () => {};
assertThrows(() => process.addSignalListener("SIGBREAK", listener));
process.removeSignalListener("SIGBREAK", listener);
