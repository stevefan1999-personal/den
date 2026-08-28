const target = new EventTarget();
const log = [];
const first = () => log.push("first");
target.addEventListener("ping", first);
target.addEventListener("ping", () => log.push("once"), { once: true });
target.addEventListener("ping", first);
target.addEventListener("ping", () => log.push("last"));
target.dispatchEvent(new Event("ping"));
target.removeEventListener("ping", first);
target.dispatchEvent(new Event("ping"));
log.join(",")
