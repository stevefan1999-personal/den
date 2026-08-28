// A handmade signal: EventTarget only ever reads `aborted`
// and listens for `abort`, so this is a faithful enough one.
globalThis.signal = new EventTarget();
signal.aborted = false;
target.addEventListener("message", listener, { signal });
