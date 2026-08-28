(() => {
                 const map = new Map([["a", 1], [2, "b"], [true, null]]);
                 const set = new Set(["x", 7, false]);
                 const out = structuredClone({ map, set });
                 return out.map instanceof Map && out.set instanceof Set
                   && JSON.stringify([...out.map]) === JSON.stringify([["a", 1], [2, "b"], [true, null]])
                   && JSON.stringify([...out.set]) === JSON.stringify(["x", 7, false]);
               })()
