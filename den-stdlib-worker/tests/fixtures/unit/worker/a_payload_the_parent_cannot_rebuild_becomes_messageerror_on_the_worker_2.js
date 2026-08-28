globalThis.worker = new Worker("./bad.js");
const seen = await new Promise((resolve) => {
  worker.onmessage = (event) => resolve(`message:${event.data}`);
  worker.onmessageerror = (event) => resolve(`${event.type}:${event.data}`);
});
worker.terminate();
seen
