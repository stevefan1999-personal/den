const target = new EventTarget();
const log = [];
target.addEventListener("ping", () => {
  log.push("first");
  target.addEventListener("ping", () => log.push("added"));
}, { once: true });
target.dispatchEvent(new Event("ping"));
target.dispatchEvent(new Event("ping"));
log.join(",")
