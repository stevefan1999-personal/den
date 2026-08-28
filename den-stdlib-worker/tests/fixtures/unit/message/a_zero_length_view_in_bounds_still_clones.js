(() => {
                 const resizable = new ArrayBuffer(8, { maxByteLength: 8 });
                 const out = structuredClone({
                   empty: new Uint8Array(new ArrayBuffer(0)),
                   emptyInResizable: new Uint8Array(resizable, 0, 0),
                   emptyView: new DataView(resizable, 8),
                 });
                 return out.empty.length === 0 && out.emptyInResizable.length === 0
                   && out.emptyView.byteLength === 0;
               })()
