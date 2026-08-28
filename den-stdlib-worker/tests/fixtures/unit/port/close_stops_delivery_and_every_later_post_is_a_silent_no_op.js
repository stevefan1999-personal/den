globalThis.log = [];
const channel = new MessageChannel();
channel.port2.onmessage = (event) => log.push(event.data);
channel.port2.close();
channel.port1.postMessage("to a closed peer");
channel.port2.postMessage("from a closed port");
