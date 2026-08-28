(() => {
                 const shared = { id: 1 };
                 const value = { first: shared, second: shared, list: [] };
                 value.self = value;
                 value.list.push(value.list);
                 const out = structuredClone(value);
                 return out.self === out && out.first === out.second
                   && out.list[0] === out.list;
               })()
