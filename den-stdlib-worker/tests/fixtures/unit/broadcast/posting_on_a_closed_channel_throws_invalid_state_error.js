(() => {
                         const channel = new BroadcastChannel("post-after-close");
                         channel.close();
                         try { channel.postMessage(1); return "no throw"; }
                         catch (error) {
                           return error instanceof DOMException ? error.name : `wrong: ${error}`;
                         }
                       })()
