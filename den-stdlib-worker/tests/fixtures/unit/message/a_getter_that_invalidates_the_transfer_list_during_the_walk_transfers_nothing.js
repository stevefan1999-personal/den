(() => {
                     const kept = new MessageChannel();
                     const closed = new MessageChannel();
                     const buffer = new ArrayBuffer(8);
                     const value = { get sneaky() { closed.port1.close(); return 1; } };
                     let name = "no throw";
                     try {
                       structuredClone(value, { transfer: [buffer, kept.port1, closed.port1] });
                     } catch (error) { name = error.name; }
                     // The port listed *before* the offending one must still
                     // hold its channel, which is only observable by
                     // transferring it: a moved-out port refuses.
                     let again = "no throw";
                     try { structuredClone(null, { transfer: [kept.port1] }); again = "transferable"; }
                     catch (error) { again = error.name; }
                     return `${name}:${buffer.detached}:${again}`;
                   })()
