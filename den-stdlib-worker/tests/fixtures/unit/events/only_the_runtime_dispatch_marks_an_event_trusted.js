const target = new EventTarget();
const seen = [];
target.addEventListener("ping", (event) => {
  seen.push(event.isTrusted);
  if (event.type === "ping") target.dispatchEvent(new Event("pong"));
});
target.addEventListener("pong", (event) => seen.push(event.isTrusted));
__natives.dispatchTrusted(target, new Event("ping"));
target.dispatchEvent(new Event("ping"));
const once = new Event("ping");
__natives.dispatchTrusted(target, once);
target.dispatchEvent(once);
seen.join(",")
