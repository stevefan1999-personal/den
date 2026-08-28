globalThis.worker = new Worker("./echo.js");
worker.postMessage("ping");
const reply = await new Promise((resolve) => {
  worker.onmessage = (event) => resolve(event.data);
});
worker.terminate();
reply
