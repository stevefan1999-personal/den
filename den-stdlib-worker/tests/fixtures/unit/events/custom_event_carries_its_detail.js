const detail = { id: 1 };
const event = new CustomEvent("ping", { detail, cancelable: true });
const empty = new CustomEvent("ping");
event.initCustomEvent("pong", true, false, "later");
`${event.detail},${event.type},${empty.detail},${empty instanceof Event},${
  Object.prototype.toString.call(empty)}`
