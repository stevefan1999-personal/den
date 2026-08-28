const target = new EventTarget();
const log = [];
target.addEventListener("ping", null);
target.addEventListener("ping", {
  handleEvent(event) { log.push(`${event.type}:${this.tag}`) },
  tag: "object",
});
target.addEventListener("ping", {});
target.addEventListener("ping", () => log.push("function"));
target.dispatchEvent(new Event("ping"));
log.join(",")
