self.onmessageerror = (event) => postMessage(`${event.type}:${event.data}`);
self.onmessage = () => postMessage("message");
