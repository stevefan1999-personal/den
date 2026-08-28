globalThis.log = [];
globalThis.sender = new BroadcastChannel("closed-receiver");
globalThis.receiver = new BroadcastChannel("closed-receiver");
receiver.onmessage = (event) => log.push(event.data);
