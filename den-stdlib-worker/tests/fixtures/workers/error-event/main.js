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
      event.error instanceof TypeError,
      event.error.stack.includes("worker.js:3:"),
    ].join(","));
  };
});
worker.postMessage("go");
globalThis.result = await failure;
worker.terminate();
