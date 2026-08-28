(() => {
                     const leaked = [];
                     const poison = {
                       configurable: true,
                       set(value) { leaked.push(value); },
                       get() { return "intercepted"; },
                     };
                     Object.defineProperty(Object.prototype, "secret", poison);
                     Object.defineProperty(Object.prototype, "cause", poison);
                     try {
                       const out = structuredClone({ secret: 5 });
                       const error = structuredClone(new Error("boom", { cause: "why" }));
                       return [leaked.length, Object.hasOwn(out, "secret"), out.secret,
                               Object.hasOwn(error, "cause"), error.cause].join();
                     } finally {
                       Reflect.deleteProperty(Object.prototype, "secret");
                       Reflect.deleteProperty(Object.prototype, "cause");
                     }
                   })()
