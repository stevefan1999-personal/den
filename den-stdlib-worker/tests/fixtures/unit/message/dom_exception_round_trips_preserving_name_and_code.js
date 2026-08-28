(() => {
                 const out = structuredClone(new DOMException("gone", "NotFoundError"));
                 return out instanceof DOMException && out.name === "NotFoundError"
                   && out.message === "gone" && out.code === 8;
               })()
