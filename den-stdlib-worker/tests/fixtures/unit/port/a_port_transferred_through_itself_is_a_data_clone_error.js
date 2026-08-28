globalThis.log = [];
globalThis.channel = new MessageChannel();
channel.port2.onmessage = (event) => {
  log.push(event.data);
  channel.port2.close();
};
try { channel.port1.postMessage(null, [channel.port1]); log.push("no throw"); }
catch (error) {
  log.push(error instanceof DOMException ? error.name : `wrong: ${error}`);
}
channel.port1.postMessage("still entangled");
