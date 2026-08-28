const target = new EventTarget();
let name = "nothing thrown";
target.addEventListener("ping", (event) => {
  try { target.dispatchEvent(event) } catch (error) { name = error.name }
});
const event = new Event("ping");
target.dispatchEvent(event);
`${name},${target.dispatchEvent(event)}`
