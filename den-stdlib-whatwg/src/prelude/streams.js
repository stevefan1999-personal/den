// Minimal WHATWG streams, enough for Blob.stream(), FileReader, and
// CompressionStream. Not a complete web-streams-polyfill: no BYOB, no
// backpressure strategy, no pipeTo abort plumbing.
(function (natives, api) {
  const wake = (waiters) => {
    const pending = waiters.splice(0);
    for (const resolve of pending) resolve();
  };

  class ReadableStream {
    constructor(source = {}) {
      this._source = source ?? {};
      this._queue = [];
      this._waiters = [];
      this._closed = false;
      this._errored = null;
      this._locked = false;
      this._pulling = false;
      const stream = this;
      this._controller = {
        enqueue(chunk) {
          if (stream._closed || stream._errored) return;
          stream._queue.push(chunk);
          wake(stream._waiters);
        },
        close() {
          stream._closed = true;
          wake(stream._waiters);
        },
        error(reason) {
          stream._errored = reason ?? new TypeError("ReadableStream errored");
          wake(stream._waiters);
        },
      };
      try {
        const started = this._source.start?.(this._controller);
        if (started && typeof started.then === "function") {
          started.catch((reason) => this._controller.error(reason));
        }
      } catch (reason) {
        this._controller.error(reason);
      }
    }

    get locked() {
      return this._locked;
    }

    async _pullIfEmpty() {
      if (
        this._pulling || this._closed || this._errored ||
        this._queue.length > 0 || typeof this._source.pull !== "function"
      ) {
        return;
      }
      this._pulling = true;
      try {
        await this._source.pull(this._controller);
      } catch (reason) {
        this._controller.error(reason);
      } finally {
        this._pulling = false;
      }
    }

    getReader() {
      if (this._locked) {
        throw new TypeError("ReadableStream is locked");
      }
      this._locked = true;
      const stream = this;
      return {
        async read() {
          for (;;) {
            if (stream._errored) throw stream._errored;
            if (stream._queue.length > 0) {
              return { value: stream._queue.shift(), done: false };
            }
            if (stream._closed) {
              return { value: undefined, done: true };
            }
            const pulled = stream._pullIfEmpty();
            if (stream._queue.length > 0 || stream._closed || stream._errored) {
              continue;
            }
            await Promise.all([
              pulled,
              new Promise((resolve) => stream._waiters.push(resolve)),
            ]);
          }
        },
        async cancel(reason) {
          try {
            await stream._source.cancel?.(reason);
          } catch {
            // Already cancelled or the source has no cancel.
          }
          stream._queue.length = 0;
          stream._closed = true;
          stream._locked = false;
          wake(stream._waiters);
        },
        releaseLock() {
          stream._locked = false;
        },
      };
    }

    pipeThrough(transform) {
      if (transform == null || transform.readable == null || transform.writable == null) {
        throw new TypeError("pipeThrough requires a { readable, writable } pair");
      }
      const writer = transform.writable.getWriter();
      const reader = this.getReader();
      (async () => {
        try {
          for (;;) {
            const { done, value } = await reader.read();
            if (done) {
              await writer.close();
              return;
            }
            await writer.write(value);
          }
        } catch (reason) {
          try {
            await writer.abort?.(reason);
          } catch {
            // The writable already failed.
          }
        }
      })();
      return transform.readable;
    }

    async cancel(reason) {
      return this.getReader().cancel(reason);
    }

    get [Symbol.toStringTag]() {
      return "ReadableStream";
    }
  }

  class WritableStream {
    constructor(sink = {}) {
      this._sink = sink ?? {};
      this._closed = false;
      this._errored = null;
      try {
        this._sink.start?.(this);
      } catch (reason) {
        this._errored = reason;
      }
    }

    getWriter() {
      const stream = this;
      return {
        write(chunk) {
          if (stream._errored) return Promise.reject(stream._errored);
          if (stream._closed) {
            return Promise.reject(new TypeError("WritableStream is closed"));
          }
          try {
            return Promise.resolve(stream._sink.write?.(chunk));
          } catch (reason) {
            stream._errored = reason;
            return Promise.reject(reason);
          }
        },
        close() {
          if (stream._errored) return Promise.reject(stream._errored);
          stream._closed = true;
          try {
            return Promise.resolve(stream._sink.close?.());
          } catch (reason) {
            stream._errored = reason;
            return Promise.reject(reason);
          }
        },
        abort(reason) {
          stream._closed = true;
          try {
            return Promise.resolve(stream._sink.abort?.(reason));
          } catch (error) {
            return Promise.reject(error);
          }
        },
      };
    }

    get [Symbol.toStringTag]() {
      return "WritableStream";
    }
  }

  class TransformStream {
    constructor(transformer = {}) {
      transformer = transformer ?? {};
      let readableController;
      this.readable = new ReadableStream({
        start(controller) {
          readableController = controller;
        },
      });
      this.writable = new WritableStream({
        write(chunk) {
          return transformer.transform?.(chunk, readableController);
        },
        close() {
          const flushed = transformer.flush?.(readableController);
          return Promise.resolve(flushed).then(() => readableController.close());
        },
        abort(reason) {
          readableController.error(reason);
        },
      });
    }

    get [Symbol.toStringTag]() {
      return "TransformStream";
    }
  }

  return { ...api, ReadableStream, WritableStream, TransformStream };
})
