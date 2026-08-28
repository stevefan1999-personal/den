self.onunhandledrejection = (event) => {
  event.preventDefault();
  postMessage([
    event instanceof PromiseRejectionEvent,
    event.type,
    event.reason.message,
    // The runtime fired it, so DOM says trusted; a script cannot forge one.
    event.isTrusted,
    event.cancelable,
    typeof event.promise,
  ].join(","));
};
Promise.reject(new Error("nobody in here either"));
