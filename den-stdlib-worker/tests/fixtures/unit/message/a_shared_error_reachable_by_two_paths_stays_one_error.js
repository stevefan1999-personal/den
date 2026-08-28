(() => {
                 const error = new TypeError("once");
                 error.cause = error;
                 const out = structuredClone({ a: error, b: [error] });
                 return out.a === out.b[0] && out.a.cause === out.a;
               })()
