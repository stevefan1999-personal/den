const worker = new Worker("./worker.js");
// A clone tag whose revival throws on the far side: a DataView
// cannot be built past the end of its buffer.
worker.postMessage({
  ["\u0000den:structured-clone"]: "DataView",
  buffer: new ArrayBuffer(4), byteOffset: 99, byteLength: 99,
});
globalThis.result = await firstMessage(worker);
worker.terminate();
