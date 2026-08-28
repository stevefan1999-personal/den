(() => {
                 const out = structuredClone({ at: new Date(1234567890123), bad: new Date(NaN) });
                 return out.at instanceof Date && out.at.getTime() === 1234567890123
                   && Number.isNaN(out.bad.getTime());
               })()
