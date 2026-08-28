self.onerror = () => { throw new Error("from onerror"); };
self.onmessage = (event) => postMessage(`alive:${event.data}`);
throw new Error("the original");
