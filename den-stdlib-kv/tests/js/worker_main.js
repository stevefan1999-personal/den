const worker = new Worker("./worker_child.js", { type: "module", name: "kv-shutdown" });
await new Promise((resolve, reject) => {
  worker.addEventListener("message", resolve, { once: true });
  worker.addEventListener("error", (event) => {
    event.preventDefault();
    reject(new Error(event.message));
  }, { once: true });
});
