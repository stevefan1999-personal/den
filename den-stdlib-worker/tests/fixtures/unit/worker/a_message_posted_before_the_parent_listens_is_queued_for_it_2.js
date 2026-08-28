globalThis.worker = new Worker("./eager.js");
await new Promise((resolve) => {
  worker.onerror = (event) => { event.preventDefault(); resolve(); };
});
// Only now is anything in this realm listening for a message.
const queued = await new Promise((resolve) => {
  worker.onmessage = (event) => resolve(event.data);
});
worker.terminate();
queued
