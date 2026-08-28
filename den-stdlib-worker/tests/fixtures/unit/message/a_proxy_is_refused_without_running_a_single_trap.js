(() => {
                 let traps = 0;
                 const proxy = new Proxy({}, new Proxy({}, { get: () => { traps += 1; return undefined; } }));
                 try { structuredClone({ proxy }); } catch { return traps === 0; }
                 return false;
               })()
