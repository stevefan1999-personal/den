const worker = new Worker("./worker.js");
worker.postMessage("ping");
globalThis.result = await firstMessage(worker);
worker.terminate();
