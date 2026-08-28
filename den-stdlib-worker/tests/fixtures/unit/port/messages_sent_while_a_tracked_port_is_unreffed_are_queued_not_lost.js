target.removeEventListener("message", listener);
channel.port1.postMessage("while unreffed 1");
channel.port1.postMessage("while unreffed 2");
