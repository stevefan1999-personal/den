globalThis.seen = [];
const record = (event) => {
  globalThis.seen.push(`${event.type}:${event.reason.message}`);
  if (globalThis.claim) event.preventDefault();
};
addEventListener("unhandledrejection", record);
addEventListener("rejectionhandled", record);
