globalThis.worker = new Worker("./worker.js", { name: "spin-terminate" });
globalThis.result = await firstMessage(worker);
