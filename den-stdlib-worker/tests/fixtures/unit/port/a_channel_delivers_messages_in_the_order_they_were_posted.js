globalThis.log = [];
const channel = new MessageChannel();
channel.port2.onmessage = (event) => {
  log.push(event.data);
  if (log.length === 3) channel.port2.close();
};
for (const item of [1, 2, 3]) channel.port1.postMessage(item);
