(() => {
                     const buffer = new ArrayBuffer(8);
                     Object.defineProperty(buffer, "transfer", {
                       value: () => { throw new TypeError("sealed"); },
                     });
                     let name = "no throw";
                     try { structuredClone(buffer, { transfer: [buffer] }); }
                     catch (error) { name = error.name; }
                     return `${name}:${buffer.detached}:${buffer.byteLength}`;
                   })()
