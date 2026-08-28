(() => {
                 const buffer = new Uint8Array([1, 2, 3]).buffer;
                 const out = structuredClone(buffer);
                 return out instanceof ArrayBuffer && out !== buffer
                   && new Uint8Array(out).join() === "1,2,3";
               })()
