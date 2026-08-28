globalThis.log = [];
const sender = new BroadcastChannel("two-receivers");
const channels = [
  new BroadcastChannel("two-receivers"),
  new BroadcastChannel("two-receivers"),
];
channels.forEach((channel, index) => {
  channel.onmessage = (event) => {
    log.push(`${index}:${event.data.value}`);
    channel.close();
  };
});
sender.postMessage({ value: 7 });
sender.close();
