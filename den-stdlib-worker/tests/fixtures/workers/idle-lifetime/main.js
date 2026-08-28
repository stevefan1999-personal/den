globalThis.worker = new Worker("./worker.js", { name: "idle-lifetime" });
globalThis.worker.postMessage("ping");
globalThis.result = await firstMessage(globalThis.worker);
