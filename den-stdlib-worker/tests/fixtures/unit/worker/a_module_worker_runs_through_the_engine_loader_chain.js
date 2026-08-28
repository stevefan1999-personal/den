globalThis.worker = new Worker("./module.js", { type: "module" });
worker.postMessage(null);
const reply = await new Promise((resolve) => {
  worker.onmessage = (event) => resolve(event.data);
});
worker.terminate();
reply
