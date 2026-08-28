const worker = new Worker("./worker.js");
globalThis.result = await firstMessage(worker);
worker.terminate();
