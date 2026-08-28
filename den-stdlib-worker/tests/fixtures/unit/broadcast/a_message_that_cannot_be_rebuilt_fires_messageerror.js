globalThis.log = [];
const sender = new BroadcastChannel("bad-payload");
const receiver = new BroadcastChannel("bad-payload");
receiver.onmessage = (event) => log.push(`message:${event.data}`);
receiver.onmessageerror = (event) => {
  log.push(`${event.type}:${event.data}`);
  receiver.close();
};
// A clone tag whose revival throws on the far side: a DataView
// cannot be built past the end of its buffer.
sender.postMessage({
  "\u0000den:structured-clone": "DataView",
  buffer: new ArrayBuffer(4), byteOffset: 99, byteLength: 99,
});
sender.close();
