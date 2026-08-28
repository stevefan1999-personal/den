const worker = new Worker("./nested/outer.js");
globalThis.result = await firstMessage(worker);
worker.terminate();
