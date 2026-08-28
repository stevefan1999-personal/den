(() => {
                     const buffer = new ArrayBuffer(4);
                     structuredClone(buffer, { transfer: [buffer] });
                     let name = "no throw";
                     try { structuredClone(buffer, { transfer: [buffer] }); }
                     catch (error) { name = error.name; }
                     return `${name}:${structuredClone({ ok: 1 }).ok}`;
                   })()
