(() => {
                 const pattern = /ab+c/gi;
                 pattern.lastIndex = 3;
                 const out = structuredClone(pattern);
                 return out instanceof RegExp && out.source === "ab+c"
                   && out.flags === "gi" && out.lastIndex === 0;
               })()
