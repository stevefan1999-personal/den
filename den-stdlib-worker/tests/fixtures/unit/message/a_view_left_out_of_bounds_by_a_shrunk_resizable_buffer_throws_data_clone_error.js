(() => {
                     const buffer = new ArrayBuffer(8, { maxByteLength: 8 });
                     const view = new Uint8Array(buffer, 4);
                     buffer.resize(0);
                     return view;
                   })()
