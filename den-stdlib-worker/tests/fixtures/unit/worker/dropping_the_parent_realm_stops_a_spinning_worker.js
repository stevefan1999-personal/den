globalThis.worker = new Worker("./spin.js", { name: "dddd" });
await new Promise((resolve) => { worker.onmessage = resolve; });
