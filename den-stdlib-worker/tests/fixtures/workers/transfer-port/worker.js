self.onmessage = (event) => {
  const port = event.data.port;
  const same = port === event.ports[0];
  port.onmessage = (message) => port.postMessage(`${message.data}/pong:${same}`);
  port.postMessage("hello from the worker");
};
