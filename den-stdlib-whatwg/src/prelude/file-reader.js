// WHATWG FileReader. Ported from txiki.js (itself adapted from Deno) onto
// den's EventTarget. Reads via Blob.arrayBuffer() rather than a byte stream
// so a missing pull() cannot strand the load.
(function (natives, api) {
  const { ProgressEvent } = api;
  const EventTarget = natives.EventTarget;

  const bytesToBase64 = (bytes) => {
    let binary = "";
    const chunk = 0x8000;
    for (let offset = 0; offset < bytes.length; offset += chunk) {
      binary += String.fromCharCode(
        ...bytes.subarray(offset, offset + chunk),
      );
    }
    if (typeof btoa === "function") return btoa(binary);
    throw new TypeError("FileReader.readAsDataURL requires btoa");
  };

  class FileReader extends EventTarget {
    static EMPTY = 0;
    static LOADING = 1;
    static DONE = 2;
    EMPTY = 0;
    LOADING = 1;
    DONE = 2;

    #aborted = null;
    #error = null;
    #result = null;
    #readyState = 0;

    get error() {
      return this.#error;
    }
    get result() {
      return this.#result;
    }
    get readyState() {
      return this.#readyState;
    }

    abort() {
      if (this.#readyState === this.EMPTY || this.#readyState === this.DONE) {
        this.#result = null;
        return;
      }
      if (this.#readyState === this.LOADING) {
        this.#readyState = this.DONE;
        this.#result = null;
      }
      if (this.#aborted !== null) this.#aborted.aborted = true;
      this.dispatchEvent(new ProgressEvent("abort", {}));
      if (this.#readyState !== this.LOADING) {
        this.dispatchEvent(new ProgressEvent("loadend", {}));
      }
    }

    readAsArrayBuffer(blob) {
      this.#readOperation(blob, { kind: "ArrayBuffer" });
    }

    readAsDataURL(blob) {
      this.#readOperation(blob, { kind: "DataUrl" });
    }

    readAsText(blob, encoding = "utf-8") {
      this.#readOperation(blob, { kind: "Text", encoding });
    }

    #readOperation(blob, opts) {
      if (this.#readyState === this.LOADING) {
        throw new DOMException("Invalid FileReader state", "InvalidStateError");
      }
      if (blob == null || typeof blob.arrayBuffer !== "function") {
        throw new TypeError("FileReader: argument is not a Blob");
      }

      this.#readyState = this.LOADING;
      this.#result = null;
      this.#error = null;
      const abortedState = this.#aborted = { aborted: false };

      queueMicrotask(() => {
        if (abortedState.aborted) return;
        this.dispatchEvent(new ProgressEvent("loadstart", {}));
      });

      blob.arrayBuffer().then((buffer) => {
        queueMicrotask(() => {
          if (abortedState.aborted) return;
          this.#readyState = this.DONE;
          const bytes = new Uint8Array(buffer);
          const size = bytes.byteLength;
          switch (opts.kind) {
            case "ArrayBuffer":
              this.#result = bytes.buffer;
              break;
            case "Text": {
              const decoder = typeof TextDecoder === "function"
                ? new TextDecoder(opts.encoding)
                : null;
              this.#result = decoder
                ? decoder.decode(bytes)
                : String.fromCharCode(...bytes);
              break;
            }
            case "DataUrl": {
              const mediaType = blob.type || "application/octet-stream";
              this.#result = `data:${mediaType};base64,${bytesToBase64(bytes)}`;
              break;
            }
          }
          this.dispatchEvent(new ProgressEvent("progress", {
            lengthComputable: true,
            loaded: size,
            total: size,
          }));
          this.dispatchEvent(new ProgressEvent("load", {
            lengthComputable: true,
            loaded: size,
            total: size,
          }));
          if (this.#readyState !== this.LOADING) {
            this.dispatchEvent(new ProgressEvent("loadend", {
              lengthComputable: true,
              loaded: size,
              total: size,
            }));
          }
        });
      }).catch((error) => {
        queueMicrotask(() => {
          if (abortedState.aborted) return;
          this.#readyState = this.DONE;
          this.#error = error;
          this.dispatchEvent(new ProgressEvent("error", {}));
          if (this.#readyState !== this.LOADING) {
            this.dispatchEvent(new ProgressEvent("loadend", {}));
          }
        });
      });
    }
  }

  const proto = FileReader.prototype;
  for (const name of ["abort", "error", "load", "loadend", "loadstart", "progress"]) {
    natives.defineEventHandler(proto, `on${name}`);
  }

  Object.defineProperty(proto, Symbol.toStringTag, {
    value: "FileReader",
    configurable: true,
  });

  return { ...api, FileReader };
})
