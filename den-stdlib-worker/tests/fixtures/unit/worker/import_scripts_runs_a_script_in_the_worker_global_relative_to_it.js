globalThis.worker = new Worker("./importer.js");
worker.postMessage(null);
const reply = await new Promise((resolve) => {
  worker.onmessage = (event) => resolve(event.data);
});
worker.terminate();
reply
