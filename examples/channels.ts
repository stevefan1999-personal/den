// cargo run -- examples/channels.ts
//
// MessageChannel is a pair of entangled ports in one realm.
// BroadcastChannel is the same name across realms (and workers).
// structuredClone is the structured-clone algorithm without a port.

export {};

const channel = new MessageChannel();
const direct = new Promise<unknown>((resolve) => {
  channel.port1.onmessage = ({ data }: MessageEvent) => resolve(data);
});
channel.port2.postMessage({ value: 42 });
console.log("message channel", await direct);
channel.port1.close();
channel.port2.close();

const sender = new BroadcastChannel("den-example");
const receiver = new BroadcastChannel("den-example");
const broadcast = new Promise<unknown>((resolve) => {
  receiver.onmessage = ({ data }: MessageEvent) => resolve(data);
});
sender.postMessage("heard");
console.log("broadcast", await broadcast);
sender.close();
receiver.close();

const cloned = structuredClone({ runtime: "den", n: 1 });
console.log("clone", cloned.runtime, cloned.n);
