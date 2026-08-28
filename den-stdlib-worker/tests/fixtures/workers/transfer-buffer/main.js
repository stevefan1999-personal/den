const buffer = new Uint8Array([7, 8, 9]).buffer;
const worker = new Worker("./worker.js");
worker.postMessage(buffer, [buffer]);
const here = `${buffer.detached}:${buffer.byteLength}`;
globalThis.result = `${here} -> ${await firstMessage(worker)}`;
worker.terminate();
