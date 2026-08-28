const inner = new Worker("./inner.js");
inner.onmessage = (event) => postMessage(`outer:${event.data}`);
self.onmessage = (event) => inner.postMessage(event.data);
