globalThis.worker = new Worker("./slow-teardown/echo.js", { name: "cccc" });
worker.postMessage("up");
await new Promise((resolve) => {
  worker.onmessage = (event) => resolve(event.data);
});
