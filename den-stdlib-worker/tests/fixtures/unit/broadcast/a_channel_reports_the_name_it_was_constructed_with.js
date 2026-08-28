(() => {
                         const named = new BroadcastChannel("with a name");
                         const coerced = new BroadcastChannel(7);
                         const report = [
                           named.name,
                           `${coerced.name}:${typeof coerced.name}`,
                           // Readonly: an accessor with no setter, so the
                           // assignment throws here rather than being ignored.
                           (() => {
                             try { named.name = "other"; return "assigned"; }
                             catch (error) { return `${error.constructor.name}:${named.name}`; }
                           })(),
                         ].join("|");
                         named.close();
                         coerced.close();
                         return report;
                       })()
