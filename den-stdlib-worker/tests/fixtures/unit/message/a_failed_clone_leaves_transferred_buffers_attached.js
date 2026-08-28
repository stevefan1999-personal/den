(() => {
                 const buffer = new ArrayBuffer(4);
                 try { structuredClone({ buffer, bad: () => {} }, { transfer: [buffer] }); }
                 catch { return buffer.detached === false && buffer.byteLength === 4; }
                 return false;
               })()
