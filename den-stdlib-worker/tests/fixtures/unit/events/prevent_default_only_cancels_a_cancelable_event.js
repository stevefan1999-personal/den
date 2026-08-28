const target = new EventTarget();
let prevented = "unset";
target.addEventListener("ping", (event) => {
  event.preventDefault();
  prevented = String(event.defaultPrevented);
});
const cancelable = target.dispatchEvent(new Event("ping", { cancelable: true }));
const plain = target.dispatchEvent(new Event("ping"));
`${cancelable},${plain},${prevented}`
