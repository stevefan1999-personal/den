try {
  process.addSignalListener("SIGINT", () => {});
  postMessage("no error");
} catch (error) {
  postMessage(error.message);
}
