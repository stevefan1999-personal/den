globalThis.log = [];
globalThis.channel = new MessageChannel();
globalThis.listener = (event) => log.push(event.data);
globalThis.target = new EventTarget();
globalThis.arm = __trackMessageListeners(target, nativeOf(channel.port2));
arm();
