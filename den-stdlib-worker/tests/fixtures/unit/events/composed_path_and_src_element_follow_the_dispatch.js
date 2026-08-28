const target = new EventTarget();
const event = new Event("ping");
const before = `${event.composedPath().length},${event.srcElement}`;
let during = "";
target.addEventListener("ping", () => {
  const path = event.composedPath();
  during = `${path.length},${path[0] === target},${event.srcElement === target}`;
});
target.dispatchEvent(event);
`${before}|${during}|${event.composedPath().length},${event.srcElement === target}`
