(() => {
                 const kinds = ["Int8Array", "Uint8Array", "Uint8ClampedArray", "Int16Array",
                                "Uint16Array", "Int32Array", "Uint32Array", "Float32Array",
                                "Float64Array", "Float16Array", "BigInt64Array", "BigUint64Array"];
                 const broken = kinds.filter((kind) => {
                   const constructor = globalThis[kind];
                   if (typeof constructor !== "function") return false;
                   const big = kind.startsWith("Big");
                   const source = new constructor(big ? [1n, 2n] : [1, 2]);
                   const out = structuredClone(source);
                   return !(out instanceof constructor) || out.length !== 2
                     || out[0] !== source[0] || out[1] !== source[1];
                 });
                 return broken.length === 0;
               })()
