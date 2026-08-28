const target = new EventTarget();
target.addEventListener("boom", (event) => event.preventDefault());
const cancelable = __natives.dispatchTrusted(target, new Event("boom", { cancelable: true }));
const plain = __natives.dispatchTrusted(target, new Event("boom"));
`${cancelable},${plain}`
