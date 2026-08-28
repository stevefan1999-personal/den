const target = new EventTarget();
const log = [];
const second = () => log.push("second");
target.addEventListener("ping", () => {
  log.push("first");
  target.removeEventListener("ping", second);
});
target.addEventListener("ping", second);
target.addEventListener("ping", () => log.push("third"));
target.dispatchEvent(new Event("ping"));
log.join(",")
