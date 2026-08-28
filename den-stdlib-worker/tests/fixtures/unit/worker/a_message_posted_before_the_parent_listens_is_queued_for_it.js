postMessage("posted before anyone listened");
// Out-of-band proof that the post above has already happened: the
// error chain is not the port, and an uncaught error does not stop
// a worker (§10.2.5).
throw new Error("the worker has posted");
