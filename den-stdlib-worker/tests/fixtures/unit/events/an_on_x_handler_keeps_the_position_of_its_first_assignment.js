class Widget extends EventTarget {}
__defineEventHandler(Widget.prototype, "onping");
const widget = new Widget();
const log = [];
widget.onping = () => log.push("one");
widget.addEventListener("ping", () => log.push("two"));
widget.onping = () => log.push("three");
widget.dispatchEvent(new Event("ping"));
widget.onping = null;
widget.dispatchEvent(new Event("ping"));
widget.onping = () => log.push("five");
widget.dispatchEvent(new Event("ping"));
log.join(",")
