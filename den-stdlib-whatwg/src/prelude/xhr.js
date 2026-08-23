// WHATWG XMLHttpRequest, ported from txiki.js polyfills/xhr.js onto fetch().
(function (natives, api) {
  const { ProgressEvent } = api;
  const EventTarget = natives.EventTarget;
  const Event = natives.Event;

  const getCharset = (mimeType) => {
    const match = /;\s*charset=(?:"([^"]+)"|([^;]+))/i.exec(mimeType);
    if (!match) return null;
    return (match[1] ?? match[2]).trim();
  };

  const makeSignal = () => {
    const signal = {
      aborted: false,
      listeners: [],
      addEventListener(type, fn) {
        if (type === "abort") this.listeners.push(fn);
      },
      abort() {
        if (this.aborted) return;
        this.aborted = true;
        for (const fn of this.listeners) fn();
      },
    };
    return signal;
  };

  class XMLHttpRequest extends EventTarget {
    static UNSENT = 0;
    static OPENED = 1;
    static HEADERS_RECEIVED = 2;
    static LOADING = 3;
    static DONE = 4;
    UNSENT = 0;
    OPENED = 1;
    HEADERS_RECEIVED = 2;
    LOADING = 3;
    DONE = 4;

    #readyState = XMLHttpRequest.UNSENT;
    #status = 0;
    #statusText = "";
    #responseURL = "";
    #responseHeaders = "";
    #responseBody = new Uint8Array(0);
    #responseType = "";
    #overrideCharset = null;
    #timeout = 0;
    #withCredentials = false;
    #method = "";
    #url = "";
    #requestHeaders = typeof Headers === "function" ? new Headers() : null;
    #signal = null;

    #setReadyState(state) {
      if (this.#readyState !== state) {
        this.#readyState = state;
        this.dispatchEvent(new Event("readystatechange"));
      }
    }

    #decodeText() {
      const label = this.#overrideCharset ??
        getCharset(this.getResponseHeader("content-type") ?? "");
      if (label && typeof TextDecoder === "function") {
        try {
          return new TextDecoder(label).decode(this.#responseBody);
        } catch {
          // Unknown encoding: UTF-8.
        }
      }
      if (typeof TextDecoder === "function") {
        return new TextDecoder().decode(this.#responseBody);
      }
      return String.fromCharCode(...this.#responseBody);
    }

    get readyState() {
      return this.#readyState;
    }

    get response() {
      if (this.#readyState !== XMLHttpRequest.DONE) {
        return this.#responseType === "" || this.#responseType === "text" ? "" : null;
      }
      switch (this.#responseType) {
        case "":
        case "text":
          return this.#decodeText();
        case "arraybuffer":
          return this.#responseBody.buffer.slice(
            this.#responseBody.byteOffset,
            this.#responseBody.byteOffset + this.#responseBody.byteLength,
          );
        case "json": {
          try {
            return JSON.parse(new TextDecoder().decode(this.#responseBody));
          } catch {
            return null;
          }
        }
        default:
          return null;
      }
    }

    get responseText() {
      if (this.#responseType !== "" && this.#responseType !== "text") {
        throw new DOMException(
          "Failed to read responseText: responseType is not text",
          "InvalidStateError",
        );
      }
      if (this.#readyState !== XMLHttpRequest.LOADING &&
        this.#readyState !== XMLHttpRequest.DONE) {
        return "";
      }
      return this.#decodeText();
    }

    get responseXML() {
      return null;
    }

    set responseType(value) {
      this.#responseType = String(value);
    }

    get responseType() {
      return this.#responseType;
    }

    get responseURL() {
      return this.#responseURL;
    }

    get status() {
      return this.#status;
    }

    get statusText() {
      return this.#statusText;
    }

    set timeout(value) {
      this.#timeout = Number(value) || 0;
    }

    get timeout() {
      return this.#timeout;
    }

    get upload() {
      return undefined;
    }

    set withCredentials(value) {
      this.#withCredentials = !!value;
    }

    get withCredentials() {
      return this.#withCredentials;
    }

    abort() {
      this.#signal?.abort();
    }

    getAllResponseHeaders() {
      return this.#responseHeaders;
    }

    getResponseHeader(name) {
      if (!this.#responseHeaders) return null;
      const lowerName = String(name).toLowerCase();
      const values = [];
      for (const line of this.#responseHeaders.split("\r\n")) {
        const colon = line.indexOf(":");
        if (colon === -1) continue;
        if (line.slice(0, colon).trim() === lowerName) {
          const val = line.slice(colon + 1).trim();
          if (val.length > 0) values.push(val);
        }
      }
      return values.length > 0 ? values.join(", ") : null;
    }

    open(method, url, asyncFlag = true) {
      if (asyncFlag === false) {
        throw new TypeError("Synchronous XHR is not supported");
      }
      this.#readyState = XMLHttpRequest.UNSENT;
      this.#status = 0;
      this.#statusText = "";
      this.#responseURL = "";
      this.#responseHeaders = "";
      this.#responseBody = new Uint8Array(0);
      this.#method = String(method);
      this.#url = String(url);
      this.#requestHeaders = typeof Headers === "function" ? new Headers() : null;
      this.#setReadyState(XMLHttpRequest.OPENED);
    }

    overrideMimeType(mimeType) {
      if (
        this.#readyState === XMLHttpRequest.LOADING ||
        this.#readyState === XMLHttpRequest.DONE
      ) {
        throw new DOMException(
          "overrideMimeType cannot be called in the LOADING or DONE state",
          "InvalidStateError",
        );
      }
      this.#overrideCharset = getCharset(String(mimeType));
    }

    setRequestHeader(name, value) {
      if (this.#readyState !== XMLHttpRequest.OPENED) {
        throw new DOMException(
          "Failed to execute setRequestHeader: the object's state must be OPENED",
          "InvalidStateError",
        );
      }
      if (this.#requestHeaders) this.#requestHeaders.set(name, value);
    }

    send(body) {
      if (this.#readyState !== XMLHttpRequest.OPENED) {
        throw new DOMException(
          "Failed to execute send: the object's state must be OPENED",
          "InvalidStateError",
        );
      }
      this.dispatchEvent(new ProgressEvent("loadstart", {}));
      const signal = makeSignal();
      this.#signal = signal;

      let payload = body;
      if (!payload) {
        payload = undefined;
      } else if (payload instanceof ArrayBuffer || ArrayBuffer.isView(payload)) {
        payload = payload instanceof ArrayBuffer
          ? new Uint8Array(payload)
          : new Uint8Array(payload.buffer, payload.byteOffset, payload.byteLength);
      }

      const init = {
        method: this.#method,
        headers: this.#requestHeaders ?? undefined,
        signal,
      };
      if (payload !== undefined && this.#method !== "GET" && this.#method !== "HEAD") {
        init.body = payload;
      }

      let timedOut = false;
      let timer = null;
      if (this.#timeout > 0 && typeof setTimeout === "function") {
        timer = setTimeout(() => {
          timedOut = true;
          signal.abort();
        }, this.#timeout);
      }

      const clearTimer = () => {
        if (timer != null && typeof clearTimeout === "function") clearTimeout(timer);
      };

      fetch(this.#url, init).then(async (response) => {
        if (signal.aborted) return;
        this.#status = response.status;
        this.#statusText = response.statusText;
        this.#responseURL = response.url;
        let headerText = "";
        if (response.headers && typeof response.headers.forEach === "function") {
          response.headers.forEach((value, name) => {
            headerText += name + ": " + value + "\r\n";
          });
        }
        this.#responseHeaders = headerText;
        this.#setReadyState(XMLHttpRequest.HEADERS_RECEIVED);
        this.#setReadyState(XMLHttpRequest.LOADING);
        const buffer = await response.arrayBuffer();
        if (signal.aborted) return;
        this.#responseBody = new Uint8Array(buffer);
        this.dispatchEvent(new ProgressEvent("progress", {
          lengthComputable: true,
          loaded: this.#responseBody.length,
          total: this.#responseBody.length,
        }));
        this.#setReadyState(XMLHttpRequest.DONE);
        clearTimer();
        this.dispatchEvent(new Event("load"));
        this.dispatchEvent(new Event("loadend"));
      }).catch(() => {
        clearTimer();
        if (timedOut) {
          this.#setReadyState(XMLHttpRequest.DONE);
          this.dispatchEvent(new Event("timeout"));
        } else if (signal.aborted) {
          this.#readyState = XMLHttpRequest.UNSENT;
          this.#status = 0;
          this.#statusText = "";
          this.dispatchEvent(new Event("abort"));
        } else {
          this.#setReadyState(XMLHttpRequest.DONE);
          this.dispatchEvent(new Event("error"));
        }
        this.dispatchEvent(new Event("loadend"));
      });
    }
  }

  const proto = XMLHttpRequest.prototype;
  for (const name of [
    "abort", "error", "load", "loadend", "loadstart", "progress",
    "readystatechange", "timeout",
  ]) {
    natives.defineEventHandler(proto, `on${name}`);
  }
  Object.defineProperty(proto, Symbol.toStringTag, {
    value: "XMLHttpRequest",
    configurable: true,
  });

  return { ...api, XMLHttpRequest };
})
