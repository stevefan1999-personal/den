self.onerror = (message, filename, lineno, colno, error) => {
  postMessage(`caught:${message}:${lineno > 0}:${error instanceof RangeError}`);
  return true;
};
self.onmessage = () => { throw new RangeError("mine"); };
postMessage("ready");
