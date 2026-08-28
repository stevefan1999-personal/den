(() => {
                     globalThis.channel = new MessageChannel();
                     const carrier = new MessageChannel();
                     channel.port2.start();
                     try {
                       carrier.port1.postMessage(null, [channel.port2]);
                       return "no throw";
                     } catch (error) {
                       return error instanceof DOMException
                         ? error.name : `wrong error: ${error}`;
                     }
                   })()
