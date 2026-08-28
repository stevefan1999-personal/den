globalThis.worker = new Worker("./spin.js");
const started = await new Promise((resolve) => {
  worker.onmessage = (event) => resolve(event.data);
});
worker.terminate();
started
