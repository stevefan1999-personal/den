(() => {
                     const value = { first: 0 };
                     // Ahead of `second` in insertion order, so the delete lands
                     // while `second` is still in the snapshot the walk took.
                     Object.defineProperty(value, "trap", {
                       enumerable: true, configurable: true,
                       get() { delete value.second; return 1; },
                     });
                     value.second = 2;
                     const out = structuredClone(value);
                     return [Object.hasOwn(out, "second"), out.trap, Object.keys(out).join("+")].join();
                   })()
