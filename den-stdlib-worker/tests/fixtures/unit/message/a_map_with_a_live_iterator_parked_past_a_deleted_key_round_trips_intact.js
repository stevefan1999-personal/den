(() => {
                 const map = new Map([["a", 1], ["b", 2], ["c", 3], ["d", 4]]);
                 const parked = map[Symbol.iterator]();
                 parked.next();
                 parked.next();
                 map.delete("b");
                 const out = structuredClone({ map, sentinel: "S" });
                 return parked !== undefined && out.sentinel === "S"
                   && JSON.stringify([...out.map]) === JSON.stringify([["a", 1], ["c", 3], ["d", 4]]);
               })()
