// Listening is not work: nothing here is a future the runtime waits for.
process.addSignalListener("SIGUSR2", () => {});
