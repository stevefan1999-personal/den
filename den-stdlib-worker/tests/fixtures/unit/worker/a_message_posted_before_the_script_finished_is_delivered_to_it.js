// Long enough that the parent's postMessage is certainly first, and
// synchronous so the queue cannot open in the middle of it.
let sum = 0;
for (let step = 0; step < 3_000_000; step += 1) sum += step;
self.onmessage = (event) => postMessage(`late:${event.data}:${sum > 0}`);
