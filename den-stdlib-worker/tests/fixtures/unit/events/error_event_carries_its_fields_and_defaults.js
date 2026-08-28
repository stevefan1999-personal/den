const event = new ErrorEvent("error", {
  message: "boom", filename: "worker.js", lineno: 3, colno: 7, error: "carried",
});
const empty = new ErrorEvent("error");
[
  event.message, event.filename, event.lineno, event.colno, event.error,
  empty.message === "", empty.lineno, empty.colno, empty.error === undefined,
].join(",")
