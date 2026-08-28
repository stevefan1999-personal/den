(() => {
                     const value = JSON.parse('{"__proto__": {"polluted": true}, "keep": 2}');
                     const out = structuredClone(value);
                     return [Object.hasOwn(out, "__proto__"),
                             out.__proto__.polluted === true,
                             Object.getPrototypeOf(out) === Object.prototype,
                             out.keep].join();
                   })()
