const inner = new Worker("./inner.js");
inner.onmessage = (event) => {
  postMessage(`outer saw: ${event.data}`);
  inner.terminate();
};
inner.onerror = (event) => postMessage(`outer failed: ${event.message}`);
