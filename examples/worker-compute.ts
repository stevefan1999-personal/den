// cargo run -- examples/worker-compute.ts
//
// One OS thread and one QuickJS runtime per Worker. The child instantiates
// WebAssembly and posts the export; the parent never shares a realm lock.

const worker = new Worker("./worker-compute-child.ts", {
  type: "module",
  name: "add",
});
const reply = new Promise<number>((resolve, reject) => {
  worker.onmessage = ({ data }: MessageEvent<number>) => resolve(data);
  worker.onerror = ({ message }: ErrorEvent) => reject(new Error(message));
});
worker.postMessage({ left: 40, right: 2 });
console.log("worker wasm add", await reply);
worker.terminate();
