globalThis.log = [];
const receiver = new BroadcastChannel("across-threads");
receiver.onmessage = (event) => {
  log.push(event.data);
  receiver.close();
};
