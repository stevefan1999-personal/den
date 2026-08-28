const workers = ["first", "second"].map((name) => new Worker("./worker.js", { name }));
await Promise.all(workers.map(firstMessage));

const mine = new BroadcastChannel("integration");
let heardMyself = false;
mine.onmessage = () => { heardMyself = true; };

const echoes = Promise.all(workers.map(firstMessage));
mine.postMessage("hello");
const heard = (await echoes).sort().join(",");

mine.close();
for (const worker of workers) worker.terminate();
globalThis.result = `${heard} self:${heardMyself}`;
