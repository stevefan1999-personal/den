globalThis.log = [];
const sender = new BroadcastChannel("isolated-a");
const other = new BroadcastChannel("isolated-b");
const same = new BroadcastChannel("isolated-a");
other.onmessage = (event) => log.push(`b:${event.data}`);
same.onmessage = (event) => {
  log.push(`a:${event.data}`);
  other.close();
  same.close();
};
sender.postMessage("only for a");
sender.close();
