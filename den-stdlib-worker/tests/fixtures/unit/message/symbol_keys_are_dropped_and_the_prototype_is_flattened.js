(() => {
                 class Point { constructor() { this.x = 1; this[Symbol("tag")] = 2; } }
                 const out = structuredClone(new Point());
                 return Object.getPrototypeOf(out) === Object.prototype
                   && Object.getOwnPropertySymbols(out).length === 0
                   && out.x === 1 && !(out instanceof Point);
               })()
