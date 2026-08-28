globalThis.log = [];
const channel = new MessageChannel();
channel.port2.onmessageerror = (event) => {
  log.push(`${event.type}:${event.data}`);
  channel.port2.close();
};
// `onmessageerror` does not enable the queue; only `onmessage`
// and `start()` do.
channel.port2.start();
// A clone tag whose revival throws on the far side: a DataView
// cannot be built past the end of its buffer.
channel.port1.postMessage({
  "\u0000den:structured-clone": "DataView",
  buffer: new ArrayBuffer(4), byteOffset: 99, byteLength: 99,
});
