const worker = new Worker("./worker.js", { type: "module" });
worker.postMessage(21);
globalThis.result = await firstMessage(worker);
worker.terminate();
