const target = new EventTarget();
const log = [];
target.addEventListener("ping", (event) => {
  log.push("first");
  event.stopImmediatePropagation();
}, { once: true });
target.addEventListener("ping", () => log.push("second"));
const returned = target.dispatchEvent(new Event("ping"));
target.dispatchEvent(new Event("ping"));
`${log.join(",")}|${returned}`
