const ports = ["first", "second"];
const event = new MessageEvent("message", { data: 42, ports });
ports.push("added after construction");
const empty = new MessageEvent("message");
[
  event.data, Object.isFrozen(event.ports), event.ports.join("+"),
  event.origin === "", event.lastEventId === "", event.source === null,
  empty.data === null, Object.isFrozen(empty.ports), empty.ports.length,
].join(",")
