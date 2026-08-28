import { assertEquals } from "den:assert";

const ticks = [];
await new Promise((resolve) => {
  const id = setInterval((label) => {
    ticks.push(label);
    if (ticks.length === 2) {
      clearInterval(id);
      resolve();
    }
  }, 10, "n");
});
assertEquals(ticks, ["n", "n"]);

let leaked = false;
const cancelled = setInterval(() => {
  leaked = true;
}, 10);
clearInterval(cancelled);
clearInterval();
await new Promise((resolve) => setTimeout(resolve, 25));
assertEquals(leaked, false);
assertEquals(typeof setInterval, "function");
assertEquals(typeof clearInterval, "function");
