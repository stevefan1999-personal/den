(() => {
                         const channel = new BroadcastChannel("port-payload");
                         const listener = new BroadcastChannel("port-payload");
                         try { channel.postMessage({ port: new MessageChannel().port1 }); return "no throw"; }
                         catch (error) {
                           return error instanceof DOMException ? error.name : `wrong: ${error}`;
                         } finally { channel.close(); listener.close(); }
                       })()
