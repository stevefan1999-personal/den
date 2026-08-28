const worker = new Worker("./worker.js", { type: "module" });
const reply = new Promise((resolve, reject) => {
  worker.onmessage = (event) => resolve(event.data);
  worker.onerror = (event) => reject(new Error(event.message));
});

worker.postMessage(21);
globalThis.embeddedWorkerAnswer = await reply;
worker.terminate();
