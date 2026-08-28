(() => {
                 const buffer = new ArrayBuffer(8);
                 const view = new DataView(buffer, 2, 4);
                 view.setUint8(0, 42);
                 const out = structuredClone({ view, bytes: new Uint8Array(buffer) });
                 return out.view instanceof DataView && out.view.byteOffset === 2
                   && out.view.byteLength === 4 && out.view.getUint8(0) === 42
                   && out.view.buffer === out.bytes.buffer;
               })()
