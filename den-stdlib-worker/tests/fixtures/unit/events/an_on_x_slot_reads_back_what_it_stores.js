class Widget extends EventTarget {}
__defineEventHandler(Widget.prototype, "onping");
const widget = new Widget();
const handler = () => {};
const initial = widget.onping;
widget.onping = handler;
const stored = widget.onping === handler;
widget.onping = "not a callback";
const primitive = widget.onping;
widget.onping = handler;
widget.onping = null;
`${initial},${stored},${primitive},${widget.onping}`
