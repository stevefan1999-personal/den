globalThis.log = [];
const channel = new MessageChannel();
channel.port2.addEventListener("scripted", (event) =>
  log.push(`scripted:${event.isTrusted}`));
channel.port2.onmessage = (event) => {
  log.push(`message:${event.isTrusted}`);
  channel.port2.dispatchEvent(new MessageEvent("scripted"));
  channel.port2.close();
};
channel.port1.postMessage(1);
