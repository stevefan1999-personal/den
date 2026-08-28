(() => {
                 const source = new Uint8Array([9, 8, 7, 6]).buffer;
                 const view = new Uint8Array(source);
                 const out = structuredClone({ buffer: source }, { transfer: [source] });
                 return new Uint8Array(out.buffer).join() === "9,8,7,6"
                   && source.detached === true && source.byteLength === 0 && view.length === 0;
               })()
