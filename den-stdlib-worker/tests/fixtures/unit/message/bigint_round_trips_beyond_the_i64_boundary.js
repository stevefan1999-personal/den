(() => {
                 const values = [0n, 1n, -1n, 2n ** 63n, -(2n ** 64n) - 1n, 2n ** 200n];
                 const out = structuredClone(values);
                 return out.every((value, index) => value === values[index]);
               })()
