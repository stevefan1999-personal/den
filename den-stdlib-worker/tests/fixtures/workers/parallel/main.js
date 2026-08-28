globalThis.spinners = [0, 1, 2]
  .map(() => new Worker("./spin.js", { name: "parallel-echo" }));
// Each spinner announces itself before entering its loop, so all
// three are provably running before the echo is asked for.
await Promise.all(spinners.map(firstMessage));

const echo = new Worker("./echo.js");
echo.postMessage("still responsive");
globalThis.result = await firstMessage(echo);
echo.terminate();
