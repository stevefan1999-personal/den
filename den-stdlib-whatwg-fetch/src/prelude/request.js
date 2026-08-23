// WHATWG Request. Ported from txiki.js polyfills/fetch/request.js and
// simplified: the body is stored as-is and converted in arrayBuffer(), so
// Blob/FormData (installed later by den:whatwg) work at call time.
(function (natives, api) {
  const { Headers } = api;
  const TO_MULTIPART = Symbol.for("den.toMultipartBlob");
  const METHODS = ["CONNECT", "DELETE", "GET", "HEAD", "OPTIONS", "PATCH", "POST", "PUT", "TRACE"];

  const normalizeMethod = (method) => {
    const upcased = String(method).toUpperCase();
    return METHODS.indexOf(upcased) > -1 ? upcased : String(method);
  };

  const encodeUtf8 = (text) => {
    if (typeof TextEncoder === "function") return new TextEncoder().encode(text);
    const bytes = new Uint8Array(text.length);
    for (let offset = 0; offset < text.length; offset++) {
      bytes[offset] = text.charCodeAt(offset) & 0xff;
    }
    return bytes;
  };

  class Request {
    #body = null;
    #bodyUsed = false;

    constructor(input, options = {}) {
      let body = options.body;

      if (input instanceof Request) {
        if (input.bodyUsed) throw new TypeError("Already read");
        this.url = input.url;
        this.credentials = input.credentials;
        this.redirect = input.redirect;
        this.method = input.method;
        this.mode = input.mode;
        this.signal = input.signal;
        if (!options.headers) this.headers = new Headers(input.headers);
        if (body === undefined) body = input.#body;
      } else {
        this.url = String(input);
      }

      this.credentials = options.credentials || this.credentials || "same-origin";
      this.redirect = options.redirect || this.redirect || "follow";
      if (options.headers || !this.headers) {
        this.headers = new Headers(options.headers);
      }
      this.method = normalizeMethod(options.method || this.method || "GET");
      this.mode = options.mode || this.mode || null;
      this.signal = options.signal || this.signal || null;
      this.referrer = null;

      if ((this.method === "GET" || this.method === "HEAD") && body) {
        throw new TypeError("Body not allowed for GET or HEAD requests");
      }

      if (typeof URLSearchParams === "function" && body instanceof URLSearchParams) {
        body = body.toString();
        if (!this.headers.has("content-type")) {
          this.headers.set("content-type", "application/x-www-form-urlencoded;charset=UTF-8");
        }
      }

      if (body != null && typeof body[TO_MULTIPART] === "function") {
        body = body[TO_MULTIPART]();
      }

      if (typeof body === "string" && !this.headers.has("content-type")) {
        this.headers.set("content-type", "text/plain;charset=UTF-8");
      } else if (
        body && typeof Blob === "function" && body instanceof Blob && body.type &&
        !this.headers.has("content-type")
      ) {
        this.headers.set("content-type", body.type);
      }

      this.#body = body ?? null;
    }

    get bodyUsed() {
      return this.#bodyUsed;
    }

    get body() {
      return null;
    }

    async arrayBuffer() {
      if (this.#bodyUsed) throw new TypeError("Already read");
      this.#bodyUsed = true;
      const body = this.#body;
      if (body == null) return new ArrayBuffer(0);
      if (typeof body === "string") return encodeUtf8(body).buffer;
      if (body instanceof ArrayBuffer) return body.slice(0);
      if (ArrayBuffer.isView(body)) {
        return body.buffer.slice(body.byteOffset, body.byteOffset + body.byteLength);
      }
      if (typeof body.arrayBuffer === "function") return body.arrayBuffer();
      return encodeUtf8(String(body)).buffer;
    }

    async text() {
      const buffer = await this.arrayBuffer();
      if (typeof TextDecoder === "function") return new TextDecoder().decode(buffer);
      return String.fromCharCode(...new Uint8Array(buffer));
    }

    async json() {
      return JSON.parse(await this.text());
    }

    async blob() {
      const buffer = await this.arrayBuffer();
      const type = this.headers.get("content-type") ?? "";
      if (typeof Blob !== "function") {
        throw new TypeError("Blob is not defined");
      }
      return new Blob([buffer], { type });
    }

    clone() {
      if (this.#bodyUsed) throw new TypeError("Already read");
      return new Request(this.url, {
        method: this.method,
        headers: new Headers(this.headers),
        body: this.#body,
        signal: this.signal,
        credentials: this.credentials,
        redirect: this.redirect,
        mode: this.mode,
      });
    }

    get [Symbol.toStringTag]() {
      return "Request";
    }
  }

  return { ...api, Request };
})
