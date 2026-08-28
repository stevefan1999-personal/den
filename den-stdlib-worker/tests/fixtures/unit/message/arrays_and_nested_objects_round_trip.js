(() => {
                 const value = { list: [1, [2, [3, { deep: "yes" }]]], flag: false };
                 const out = structuredClone(value);
                 return JSON.stringify(out) === JSON.stringify(value) && out.list !== value.list;
               })()
