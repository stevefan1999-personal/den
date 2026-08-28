(() => {
                 const names = ["Error", "EvalError", "RangeError", "ReferenceError",
                                "SyntaxError", "TypeError", "URIError"];
                 return names.every((name) => {
                   const out = structuredClone(new globalThis[name]("boom"));
                   return out instanceof globalThis[name] && out.name === name
                     && out.message === "boom" && typeof out.stack === "string";
                 });
               })()
