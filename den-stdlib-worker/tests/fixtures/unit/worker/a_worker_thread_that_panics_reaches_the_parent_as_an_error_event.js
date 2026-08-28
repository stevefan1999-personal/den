globalThis.worker = new Worker("./panicking-host/echo.js");
const reported = await new Promise((resolve) => {
  worker.onerror = (event) => {
    event.preventDefault();
    resolve(event.message);
  };
});
reported
