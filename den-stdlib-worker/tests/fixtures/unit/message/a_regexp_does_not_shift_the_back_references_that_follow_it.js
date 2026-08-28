(() => {
                 const buffer = new Uint8Array([1, 2, 3, 4]).buffer;
                 const out = structuredClone({
                   pattern: /ab+c/gi,
                   view: new Uint16Array(buffer, 2, 1),
                   dataView: new DataView(buffer, 1, 2),
                 });
                 return out.pattern.source === "ab+c"
                   && out.view.buffer === out.dataView.buffer
                   && out.view.byteOffset === 2 && out.dataView.byteLength === 2;
               })()
