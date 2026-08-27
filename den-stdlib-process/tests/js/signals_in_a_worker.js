import { assert } from "den:assert";

const worker = new Worker("./signals_in_a_worker_child.js", { type: "module" });
const message = await new Promise((resolve, reject) => {
  worker.addEventListener("message", (event) => resolve(event.data), { once: true });
  worker.addEventListener("error", (event) => {
    event.preventDefault();
    reject(new Error(event.message));
  }, { once: true });
});
worker.terminate();

assert(
  message === "signal listeners are not available in workers",
  `worker said ${JSON.stringify(message)}`,
);
