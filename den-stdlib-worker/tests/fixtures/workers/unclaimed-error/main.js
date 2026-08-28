const worker = new Worker("./worker.js");
const alive = new Promise((resolve) => {
  worker.onmessage = (event) => resolve(event.data);
});
worker.postMessage("throw");
worker.postMessage("ping");
globalThis.result = await alive;
worker.terminate();
