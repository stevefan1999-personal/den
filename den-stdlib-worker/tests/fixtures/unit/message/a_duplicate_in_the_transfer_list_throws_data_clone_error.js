(() => {
                     const buffer = new ArrayBuffer(4);
                     try { structuredClone(buffer, { transfer: [buffer, buffer] }); return "no throw"; }
                     catch (error) { return error.name; }
                   })()
