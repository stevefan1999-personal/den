const worker = new Worker("./child.js", { type: "module" });
globalThis.mappedWorkerAnswer = await new Promise((resolve, reject) => {
  worker.onmessage = (event) => resolve(event.data);
  worker.onerror = (event) => reject(new Error(event.message));
});
worker.terminate();
