globalThis.seen = "nothing";
const worker = new Worker("./child.js");
worker.onerror = (event) => {
  event.preventDefault();
  globalThis.seen = event.message;
  worker.terminate();
};
