globalThis.worker = new Worker("./spin.js", { name: "tttt" });
await new Promise((resolve) => {
  worker.onmessage = (event) => resolve(event.data);
});
