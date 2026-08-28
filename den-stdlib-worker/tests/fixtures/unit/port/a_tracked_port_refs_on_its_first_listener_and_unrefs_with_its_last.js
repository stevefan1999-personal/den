globalThis.second = () => {};
target.addEventListener("message", second);
target.removeEventListener("message", listener);
