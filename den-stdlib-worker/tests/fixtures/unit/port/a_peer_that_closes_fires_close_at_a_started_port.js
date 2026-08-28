globalThis.log = [];
globalThis.channel = new MessageChannel();
channel.port2.onclose = (event) => {
  log.push(`onclose:${event.type}:${event.isTrusted}`);
  channel.port2.postMessage("after close");
};
channel.port2.addEventListener("close", () => log.push("listener"));
channel.port2.start();
channel.port1.close();
