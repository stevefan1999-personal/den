globalThis.worker = new Worker("./close.js");
const last = await new Promise((resolve) => {
  worker.onmessage = (event) => resolve(event.data);
});
last
