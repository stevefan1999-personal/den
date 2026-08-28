globalThis.claim = true;
Promise.reject(new Error("claimed by the realm"));
undefined;
