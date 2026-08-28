(() => {
                 const buffer = new ArrayBuffer(8);
                 const value = { buffer, head: new Uint8Array(buffer, 2, 4), all: new Uint8Array(buffer) };
                 value.head.set([9, 8, 7, 6]);
                 const out = structuredClone(value);
                 return out.head.byteOffset === 2 && out.head.length === 4
                   && out.head.buffer === out.buffer && out.all.buffer === out.buffer
                   && out.head.join() === "9,8,7,6";
               })()
