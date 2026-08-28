globalThis.worker = new Worker("./suppress.js");
globalThis.errors = 0;
worker.onerror = () => { errors += 1; };
const inbox = [];
const next = () => new Promise((resolve) => { worker.onmessage = (event) => resolve(event.data); });
inbox.push(await next());
// A second round trip: a fault sent right after the error
// dispatch would have arrived by now, so `errors` is a real
// observation and not a race.
worker.postMessage("ping");
inbox.push(await next());
worker.terminate();
`${inbox.join("|")}|${errors}`
