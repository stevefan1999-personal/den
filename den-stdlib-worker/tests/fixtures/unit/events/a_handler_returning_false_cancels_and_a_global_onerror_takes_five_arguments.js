const plain = new EventTarget();
__defineEventHandler(plain, "onping");
plain.onping = () => false;
const cancelledByFalse = plain.dispatchEvent(new Event("ping", { cancelable: true }));

const global = new EventTarget();
__defineEventHandler(global, "onerror", true);
let seen = "unset";
global.onerror = (message, filename, lineno, colno, error) => {
  seen = [message, filename, lineno, colno, error].join(",");
  return true;
};
const errorEvent = new ErrorEvent("error", {
  cancelable: true, message: "boom", filename: "worker.js",
  lineno: 3, colno: 7, error: "carried",
});
const cancelledByTrue = global.dispatchEvent(errorEvent);
`${cancelledByFalse}|${seen}|${cancelledByTrue}`
