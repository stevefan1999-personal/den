globalThis.log = [];
const channel = new MessageChannel();
channel.port2.onmessage = (event) => {
  log.push(event.data);
  channel.port2.close();
};
try { channel.port1.postMessage(() => {}); }
catch (error) { log.push(error.name); }
channel.port1.postMessage("still usable");
