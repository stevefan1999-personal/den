const channel = new MessageChannel();
const worker = new Worker("./worker.js");
// `addEventListener` does not enable a port's queue (HTML §9.4.4):
// only `onmessage` or an explicit `start()` does.
channel.port1.start();
const greeting = firstMessage(channel.port1);
worker.postMessage({ port: channel.port2 }, [channel.port2]);
const first = await greeting;
const reply = firstMessage(channel.port1);
channel.port1.postMessage("ping");
globalThis.result = `${first} | ${await reply}`;
channel.port1.close();
worker.terminate();
