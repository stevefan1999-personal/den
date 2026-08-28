(() => {
                 let calls = 0;
                 const value = { get computed() { calls += 1; return { deep: true }; } };
                 const out = structuredClone(value);
                 return calls === 1 && out.computed.deep === true
                   && Object.getOwnPropertyDescriptor(out, "computed").value !== undefined;
               })()
