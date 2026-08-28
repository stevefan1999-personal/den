globalThis.log = [];
globalThis.channel = new MessageChannel();
channel.port1.postMessage("early");
channel.port2.addEventListener("message", (event) => {
  log.push(event.data);
  channel.port2.close();
});
