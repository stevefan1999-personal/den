(() => {
             const shared = { id: 7 };
             const value = { when: new Date(1000), why: new TypeError("boom"),
                             pair: new Map([["k", shared]]), also: shared,
                             bytes: new Uint8Array([1, 2, 3]) };
             value.self = value;
             return value;
           })()
