const worker = new Worker("./worker.js", { name: "close-inside" });
globalThis.result = await firstMessage(worker);
