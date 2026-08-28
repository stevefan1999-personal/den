globalThis.worker = new Worker("./reentrant.js");
globalThis.errors = [];
worker.onerror = (event) => {
  event.preventDefault();
  errors.push(event.message);
};
const next = () => new Promise((resolve) => {
  worker.onmessage = (event) => resolve(event.data);
});
// Two round trips before `errors` is read: a fault sent at any
// point during the report would have arrived by then, so the
// count is an observation rather than a race.
worker.postMessage("one");
const first = await next();
worker.postMessage("two");
const second = await next();
worker.terminate();
`${first}|${second}|${errors.join(",")}`
