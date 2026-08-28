self.onmessage = (event) => {
  const delay = event.data;
  setTimeout(() => {
    postMessage({
      delay,
      epoch: Temporal.Now.instant().epochMilliseconds,
    });
  }, delay);
};
