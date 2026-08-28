globalThis.log = [];
const channel = new MessageChannel();
const buffer = new Uint8Array([1, 2, 3]).buffer;
channel.port2.onmessage = (event) => {
  log.push(new Uint8Array(event.data).join("-"));
  channel.port2.close();
};
channel.port1.postMessage(buffer, { transfer: [buffer] });
log.push(`detached:${buffer.detached}`);
