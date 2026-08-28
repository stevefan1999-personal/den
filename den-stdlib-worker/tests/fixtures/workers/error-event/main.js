const worker = new Worker("./worker.js");
await firstMessage(worker);
const failure = new Promise((resolve) => {
  worker.onerror = (event) => {
    event.preventDefault();
    resolve([
      event instanceof ErrorEvent,
      event.type,
      event.message,
      event.filename.endsWith("/worker.js"),
      event.lineno,
      // v1 divergence (docs/research/08 §1.4): an Error does not
      // serialise, so only the location crosses the thread.
      typeof event.error,
    ].join(","));
  };
});
worker.postMessage("go");
globalThis.result = await failure;
worker.terminate();
