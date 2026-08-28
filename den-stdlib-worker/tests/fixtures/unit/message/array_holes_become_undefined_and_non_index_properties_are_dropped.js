(() => {
                 const value = [1, , 3];
                 value.label = "extra";
                 const out = structuredClone(value);
                 return out.length === 3 && 1 in out && out[1] === undefined
                   && out.label === undefined;
               })()
