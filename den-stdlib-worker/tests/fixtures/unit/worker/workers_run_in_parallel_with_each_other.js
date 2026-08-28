globalThis.spinner = new Worker("./spin.js");
globalThis.echo = new Worker("./echo.js");
await new Promise((resolve) => { spinner.onmessage = () => resolve(); });
echo.postMessage("parallel");
const reply = await new Promise((resolve) => {
  echo.onmessage = (event) => resolve(event.data);
});
spinner.terminate();
echo.terminate();
reply
