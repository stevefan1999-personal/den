globalThis.ticks = 0;
const handle = setInterval(() => {
  if (++globalThis.ticks >= 2) clearInterval(handle);
}, 0);
setTimeout(() => {
  globalThis.timedOut = true;
}, 0);
undefined;
