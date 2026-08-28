const firstMessage = (target) => new Promise((resolve, reject) => {
  target.addEventListener("message", (event) => resolve(event.data), { once: true });
  target.addEventListener("error", (event) => {
    event.preventDefault();
    reject(new Error(`${event.message} (${event.filename}:${event.lineno})`));
  }, { once: true });
});
