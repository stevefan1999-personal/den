const target = new EventTarget();
const log = [];
target.addEventListener("ping", () => {
  log.push("threw");
  throw new Error("reported to stderr, not to the caller");
});
target.addEventListener("ping", () => log.push("after"));
const returned = target.dispatchEvent(new Event("ping"));
`${log.join(",")}|${returned}`
