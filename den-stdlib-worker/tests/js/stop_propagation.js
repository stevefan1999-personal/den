import { assertEquals } from "den:assert";

const log = [];
const target = new EventTarget();
target.addEventListener("ping", (event) => {
  event.stopPropagation();
  log.push("first");
});
target.addEventListener("ping", () => log.push("second"));
const dispatched = target.dispatchEvent(new Event("ping"));
log.push(`dispatched:${dispatched}`);

const immediate = new EventTarget();
immediate.addEventListener("ping", (event) => {
  event.stopImmediatePropagation();
  log.push("only");
});
immediate.addEventListener("ping", () => log.push("never"));
immediate.dispatchEvent(new Event("ping"));
assertEquals(log.join(","), "first,second,dispatched:true,only");

const again = [];
const shared = new EventTarget();
const event = new Event("ping");
let stop = true;
shared.addEventListener("ping", (seen) => {
  if (stop) seen.stopImmediatePropagation();
  again.push("first");
});
shared.addEventListener("ping", () => again.push("second"));
shared.dispatchEvent(event);
stop = false;
shared.dispatchEvent(event);
assertEquals(again.join(","), "first,first,second");
