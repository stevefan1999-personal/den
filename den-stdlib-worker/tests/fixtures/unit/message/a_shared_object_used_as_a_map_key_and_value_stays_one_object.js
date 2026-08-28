(() => {
                 const key = { k: 1 };
                 const map = new Map([[key, key]]);
                 const out = structuredClone({ map, key });
                 const [[outKey, outValue]] = [...out.map];
                 return outKey === outValue && outKey === out.key;
               })()
