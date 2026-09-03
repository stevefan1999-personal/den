import { assert, assertEquals } from "den:assert";

// den:whatwg installs CloseEvent/ProgressEvent after den:worker has defined
// Event, so both constructors always find the Event global and hand back a real
// Event. A plain-object fallback would satisfy none of this: dispatchEvent
// rejects anything that is not a native Event.
const progress = new ProgressEvent("progress", {
  lengthComputable: true,
  loaded: 3,
  total: 9,
});
assert(progress instanceof Event);
assertEquals(
  [progress.type, progress.lengthComputable, progress.loaded, progress.total].join("|"),
  "progress|true|3|9",
);
const bare = new ProgressEvent("x");
assertEquals([bare.lengthComputable, bare.loaded, bare.total].join("|"), "false|0|0");

const close = new CloseEvent("close", { code: 1001, reason: "bye", wasClean: true });
assert(close instanceof Event);
assertEquals(
  [close.type, close.code, close.reason, close.wasClean].join("|"),
  "close|1001|bye|true",
);
const bareClose = new CloseEvent("close");
assertEquals([bareClose.code, bareClose.reason, bareClose.wasClean].join("|"), "0||false");

const target = new EventTarget();
let seen = null;
target.addEventListener("progress", (event) => {
  seen = [event.type, event.loaded].join("|");
});
target.dispatchEvent(progress);
assertEquals(seen, "progress|3");
