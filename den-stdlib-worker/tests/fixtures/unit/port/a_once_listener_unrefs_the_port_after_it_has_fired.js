target.addEventListener("message", listener, { once: true });
channel.port1.postMessage("only one");
