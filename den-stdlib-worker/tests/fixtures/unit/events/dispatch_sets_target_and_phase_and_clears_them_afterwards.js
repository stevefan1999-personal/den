const target = new EventTarget();
let inside = "unset";
target.addEventListener("ping", function (event) {
  inside = [
    event.target === target, event.currentTarget === target,
    event.eventPhase === Event.AT_TARGET, this === target, event.isTrusted,
  ].join(",");
});
const event = new Event("ping");
target.dispatchEvent(event);
`${inside}|${event.target === target},${event.currentTarget},${event.eventPhase}`
