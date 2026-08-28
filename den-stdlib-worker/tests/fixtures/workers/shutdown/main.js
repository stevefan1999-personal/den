const spinner = new Worker("./spin.js", { name: "shutdown-join" });
// A parked worker: started, idle, waiting for a message that never
// comes. It has no interrupt to observe, so only the join ends it.
const parked = new Worker("./echo.js", { name: "shutdown-join" });
parked.onmessage = () => {};
globalThis.result = await firstMessage(spinner);
