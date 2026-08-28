type Ping = { value: number };
self.onmessage = (event: MessageEvent) => {
  const ping = event.data as Ping;
  postMessage(`classic:${ping.value}`);
};
