(() => {
                         const channel = new MessageChannel();
                         const immutable = new Uint8Array([1, 2, 3]).buffer.sliceToImmutable();
                         try {
                           channel.port1.postMessage(immutable, [immutable]);
                           return "no throw";
                         } catch (error) {
                           return error instanceof DOMException
                             ? `${error.name}:${immutable.byteLength}` : `wrong: ${error}`;
                         } finally { channel.port1.close(); channel.port2.close(); }
                       })()
