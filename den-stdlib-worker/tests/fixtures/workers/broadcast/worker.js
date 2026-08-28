const channel = new BroadcastChannel("integration");
channel.onmessage = (event) => {
  postMessage(`${name}:${event.data}`);
  channel.close();
};
// Only now may the parent broadcast: a channel that is not yet
// constructed is not yet subscribed, and the fan-out has no backlog.
postMessage("ready");
