globalThis.log = [];
const sender = new BroadcastChannel("sender-excluded");
const listener = new BroadcastChannel("sender-excluded");
sender.onmessage = (event) => log.push(`sender:${event.data}`);
listener.onmessage = (event) => {
  log.push(`listener:${event.data}`);
  sender.close();
  listener.close();
};
sender.postMessage("ping");
