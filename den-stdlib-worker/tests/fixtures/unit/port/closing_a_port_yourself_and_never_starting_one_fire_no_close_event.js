globalThis.log = [];
const own = new MessageChannel();
own.port2.onclose = () => log.push("own");
own.port2.start();
own.port2.close();

const unstarted = new MessageChannel();
unstarted.port2.addEventListener("close", () => log.push("unstarted"));
unstarted.port1.close();
