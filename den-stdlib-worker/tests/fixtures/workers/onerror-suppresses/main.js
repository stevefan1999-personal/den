const worker = new Worker("./worker.js");
await firstMessage(worker);
let escaped = false;
worker.onerror = () => { escaped = true; };
worker.postMessage("go");
const caught = await firstMessage(worker);
// Still alive after handling its own error.
worker.postMessage("again");
const again = await firstMessage(worker);
globalThis.result = `${caught} ${again} escaped:${escaped}`;
worker.terminate();
