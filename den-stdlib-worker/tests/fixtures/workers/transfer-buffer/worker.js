self.onmessage = (event) => {
  const bytes = new Uint8Array(event.data);
  postMessage(`${event.data.byteLength}:${bytes.join("-")}`);
};
