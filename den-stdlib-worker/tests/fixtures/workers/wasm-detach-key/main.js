const memory = new WebAssembly.Memory({ initial: 1 });
const buffer = memory.buffer;
const worker = new Worker("./worker.js");
const refuse = (attempt) => {
  try { attempt(); return "no throw"; }
  catch (error) {
    return error instanceof DOMException ? error.name : `wrong: ${error}`;
  }
};
const posted = refuse(() => worker.postMessage(buffer, [buffer]));
const cloned = refuse(() => structuredClone(buffer, { transfer: [buffer] }));

// The memory is still there and still writable: nothing was
// detached and nothing was freed.
const bytes = new Uint8Array(memory.buffer);
bytes[0] = 42;
globalThis.result = [
  posted,
  cloned,
  `detached:${buffer.detached}`,
  `bytes:${memory.buffer.byteLength}`,
  `readback:${new Uint8Array(memory.buffer)[0]}`,
].join("|");
worker.terminate();
