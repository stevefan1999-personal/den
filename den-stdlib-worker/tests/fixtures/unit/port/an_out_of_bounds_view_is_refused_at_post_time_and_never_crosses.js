globalThis.log = [];
const channel = new MessageChannel();
channel.port2.onmessage = () => log.push("message");
channel.port2.addEventListener("messageerror", () => log.push("messageerror"));
const buffer = new ArrayBuffer(8, { maxByteLength: 8 });
const view = new Uint8Array(buffer, 4);
buffer.resize(0);
try { channel.port1.postMessage(view); }
catch (error) { log.push(`${error.name}`); }
channel.port1.postMessage("still usable");
