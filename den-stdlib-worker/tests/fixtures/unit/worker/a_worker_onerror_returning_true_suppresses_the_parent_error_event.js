self.onmessage = (event) => postMessage(`pong:${event.data}`);
self.onerror = function (message, filename, lineno, colno, error) {
  postMessage(`caught:${message}:${arguments.length}:${error}`);
  return true;
};
throw new Error("hidden");
