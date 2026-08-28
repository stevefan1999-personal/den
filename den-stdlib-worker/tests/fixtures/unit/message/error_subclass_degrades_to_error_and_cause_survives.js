(() => {
                 class MyError extends Error {}
                 const value = new MyError("outer", { cause: new RangeError("inner") });
                 const out = structuredClone(value);
                 return out.constructor === Error && out.name === "Error"
                   && out.message === "outer" && out.cause instanceof RangeError
                   && out.cause.message === "inner";
               })()
