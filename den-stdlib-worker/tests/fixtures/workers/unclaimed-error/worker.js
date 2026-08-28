self.onmessage = (event) => {
  if (event.data === "throw") throw new Error("unclaimed worker failure");
  postMessage("alive");
};
