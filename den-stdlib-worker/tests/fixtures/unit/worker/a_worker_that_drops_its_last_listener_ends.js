self.onmessage = (event) => {
  if (event.data === "stop") {
    self.onmessage = null;
    return;
  }
  postMessage(`echo:${event.data}`);
};
