// A once listener is retired by the dispatch that runs it, *before* it runs.
// One that re-registers itself from inside has to be seen as a listener again
// — the pump it just unreffed must come back for the next message.
globalThis.relisten = (event) => {
  log.push(event.data);
  target.addEventListener("message", relisten, { once: true });
};
target.addEventListener("message", relisten, { once: true });
channel.port1.postMessage("first");
channel.port1.postMessage("second");
