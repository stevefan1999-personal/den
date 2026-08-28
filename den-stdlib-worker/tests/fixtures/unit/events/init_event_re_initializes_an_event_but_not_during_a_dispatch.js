const target = new EventTarget();
const event = new Event("ping", { cancelable: true });
event.preventDefault();
event.initEvent("pong", true, false);
const reset = `${event.type},${event.bubbles},${event.cancelable},${
  event.defaultPrevented},${event.target}`;
let midDispatch = "";
target.addEventListener("pong", () => {
  event.initEvent("nope");
  midDispatch = event.type;
});
target.dispatchEvent(event);
let arity = "no throw";
try { event.initEvent(); } catch (error) { arity = error.constructor.name }
`${reset}|${midDispatch}|${arity}`
