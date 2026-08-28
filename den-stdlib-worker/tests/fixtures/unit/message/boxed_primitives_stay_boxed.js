(() => {
                 const out = structuredClone([Object(1), Object("s"), Object(true), Object(5n)]);
                 return out.map((v) => typeof v).join() === "object,object,object,object"
                   && out[0].valueOf() === 1 && out[1].valueOf() === "s"
                   && out[2].valueOf() === true && out[3].valueOf() === 5n;
               })()
