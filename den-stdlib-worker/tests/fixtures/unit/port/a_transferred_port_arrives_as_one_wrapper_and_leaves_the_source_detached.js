globalThis.log = [];
const outer = new MessageChannel();
const inner = new MessageChannel();
outer.port2.onmessage = (event) => {
  const moved = event.ports[0];
  // `restore` splices the wrapper into the data graph while Rust
  // hands the same NativePort back for `ports`: one wrapper.
  log.push(`same:${event.data.port === moved}`);
  log.push(`fresh:${moved !== inner.port2}`);
  moved.onmessage = (relayed) => {
    log.push(`through:${relayed.data}`);
    moved.close();
    outer.port2.close();
  };
  inner.port1.postMessage("relayed");
};
outer.port1.postMessage({ port: inner.port2 }, [inner.port2]);
// The source is detached now, so this goes nowhere and throws
// nothing.
inner.port2.postMessage("into the void");
