(() => {
                 const value = { u: undefined, n: null, t: true, i: 42, f: 1.5,
                                 s: "héllo😀", zero: -0, nan: NaN, inf: -Infinity };
                 const out = structuredClone(value);
                 return out.u === undefined && out.n === null && out.t === true
                   && out.i === 42 && out.f === 1.5 && Object.is(out.zero, -0)
                   && Number.isNaN(out.nan) && out.inf === -Infinity && out.s === value.s;
               })()
