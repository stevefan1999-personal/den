globalThis.worker = new Worker("./throw.js");
const reported = await new Promise((resolve) => {
  worker.onerror = (event) => {
    // Uncancelled, this would be printed to stderr by the last
    // step of the chain.
    event.preventDefault();
    resolve([
      event.message,
      event.filename.endsWith("throw.js"),
      event.lineno,
      event.error instanceof TypeError,
      event.error.stack.includes("throw.js:3:"),
    ].join("|"));
  };
});
worker.terminate();
reported
