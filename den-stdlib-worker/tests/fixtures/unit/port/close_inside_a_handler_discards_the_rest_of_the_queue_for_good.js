globalThis.log = [];
globalThis.channel = new MessageChannel();
channel.port2.onmessage = (event) => {
  log.push(event.data);
  channel.port2.close();
};
for (const item of ["first", "second", "third"]) {
  channel.port1.postMessage(item);
}
