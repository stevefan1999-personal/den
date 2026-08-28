globalThis.worker = new Worker("./fickle.js");
worker.postMessage("ping");
const reply = await new Promise((resolve) => {
  worker.onmessage = (event) => resolve(event.data);
});
worker.postMessage("stop");
reply
