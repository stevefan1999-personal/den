globalThis.worker = new Worker("./late.js");
worker.postMessage("early");
const reply = await new Promise((resolve) => {
  worker.onmessage = (event) => resolve(event.data);
});
worker.terminate();
reply
