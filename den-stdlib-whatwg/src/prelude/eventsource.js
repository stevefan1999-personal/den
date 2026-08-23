// WHATWG EventSource (SSE), ported from txiki.js onto fetch().
// https://html.spec.whatwg.org/multipage/server-sent-events.html
(function (natives, api) {
  const EventTarget = natives.EventTarget;
  const Event = natives.Event;
  const DEFAULT_RECONNECT_TIME = 3000;

  const makeSignal = () => ({
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
  });

  class EventSource extends EventTarget {
    static CONNECTING = 0;
    static OPEN = 1;
    static CLOSED = 2;
    CONNECTING = 0;
    OPEN = 1;
    CLOSED = 2;

    #url;
    #withCredentials;
    #readyState;
    #abortSignal = null;
    #pending = null;
    #reconnectTimer = null;
    #reconnectTime = DEFAULT_RECONNECT_TIME;
    #origin = "";
    #buffer = "";
    #dataBuffer = "";
    #eventTypeBuffer = "";
    #lastEventId = "";
    #lastEventIdBuffer = "";

    constructor(url, options = {}) {
      super();
      this.#url = (() => {
        if (typeof URL === "function") {
          try {
            return new URL(url).href;
          } catch {
            throw new DOMException(`Cannot open an EventSource to '${url}'.`, "SyntaxError");
          }
        }
        const href = String(url);
        if (!/^https?:\/\//i.test(href)) {
          throw new DOMException(`Cannot open an EventSource to '${url}'.`, "SyntaxError");
        }
        return href;
      })();
      this.#withCredentials = Boolean(options?.withCredentials);
      this.#readyState = EventSource.CONNECTING;
      Promise.resolve().then(() => this.#start());
    }

    get url() {
      return this.#url;
    }
    get withCredentials() {
      return this.#withCredentials;
    }
    get readyState() {
      return this.#readyState;
    }

    close() {
      this.#readyState = EventSource.CLOSED;
      this.#abortSignal?.abort();
      if (this.#reconnectTimer !== null && typeof clearTimeout === "function") {
        clearTimeout(this.#reconnectTimer);
        this.#reconnectTimer = null;
      }
    }

    #start() {
      if (this.#readyState === EventSource.CLOSED) return;
      this.#buffer = "";
      this.#dataBuffer = "";
      this.#eventTypeBuffer = "";
      this.#lastEventIdBuffer = this.#lastEventId;

      const signal = makeSignal();
      this.#abortSignal = signal;
      const headers = {
        Accept: "text/event-stream",
        "Cache-Control": "no-cache",
      };
      if (this.#lastEventId !== "") headers["Last-Event-ID"] = this.#lastEventId;

      this.#pending = fetch(this.#url, { headers, signal })
        .then((response) => {
          if (this.#readyState === EventSource.CLOSED) {
            response._cancelBody?.();
            return "";
          }
          const mime = (response.headers.get("content-type") ?? "")
            .split(";", 1)[0]
            .trim()
            .toLowerCase();
          if (response.status !== 200 || mime !== "text/event-stream") {
            response._cancelBody?.();
            this.#failConnection();
            return "";
          }
          this.#readyState = EventSource.OPEN;
          try {
            this.#origin = typeof URL === "function"
              ? new URL(response.url).origin
              : this.#url;
          } catch {
            this.#origin = this.#url;
          }
          this.dispatchEvent(new Event("open"));
          return response.text();
        })
        .then((text) => {
          if (this.#readyState === EventSource.CLOSED || text === "") return;
          this.#feed(text);
          this.#reconnect();
        })
        .catch(() => this.#reconnect());
    }

    #reconnect() {
      if (this.#readyState === EventSource.CLOSED) return;
      this.#readyState = EventSource.CONNECTING;
      this.dispatchEvent(new Event("error"));
      if (this.#readyState === EventSource.CLOSED) return;
      if (typeof setTimeout !== "function") return;
      this.#reconnectTimer = setTimeout(() => {
        this.#reconnectTimer = null;
        if (this.#readyState === EventSource.CLOSED) return;
        this.#start();
      }, this.#reconnectTime);
    }

    #failConnection() {
      if (this.#readyState === EventSource.CLOSED) return;
      this.#readyState = EventSource.CLOSED;
      this.dispatchEvent(new Event("error"));
    }

    #feed(chunk) {
      this.#buffer += chunk;
      const buffer = this.#buffer;
      const len = buffer.length;
      let pos = 0;
      let lineStart = 0;
      while (pos < len) {
        const c = buffer.charCodeAt(pos);
        if (c === 0x0a) {
          this.#processLine(buffer.slice(lineStart, pos));
          pos += 1;
          lineStart = pos;
        } else if (c === 0x0d) {
          if (pos === len - 1) break;
          this.#processLine(buffer.slice(lineStart, pos));
          pos += buffer.charCodeAt(pos + 1) === 0x0a ? 2 : 1;
          lineStart = pos;
        } else {
          pos += 1;
        }
      }
      this.#buffer = buffer.slice(lineStart);
    }

    #processLine(line) {
      if (line === "") {
        this.#dispatchMessage();
        return;
      }
      if (line.charCodeAt(0) === 0x3a) return;
      const colon = line.indexOf(":");
      let field;
      let value;
      if (colon === -1) {
        field = line;
        value = "";
      } else {
        field = line.slice(0, colon);
        value = line.slice(colon + 1);
        if (value.charCodeAt(0) === 0x20) value = value.slice(1);
      }
      this.#processField(field, value);
    }

    #processField(field, value) {
      switch (field) {
        case "event":
          this.#eventTypeBuffer = value;
          break;
        case "data":
          this.#dataBuffer += value + "\n";
          break;
        case "id":
          if (!value.includes("\u0000")) this.#lastEventIdBuffer = value;
          break;
        case "retry":
          if (/^[0-9]+$/.test(value)) this.#reconnectTime = parseInt(value, 10);
          break;
        default:
          break;
      }
    }

    #dispatchMessage() {
      this.#lastEventId = this.#lastEventIdBuffer;
      if (this.#dataBuffer === "") {
        this.#eventTypeBuffer = "";
        return;
      }
      let data = this.#dataBuffer;
      if (data.charCodeAt(data.length - 1) === 0x0a) data = data.slice(0, -1);
      const type = this.#eventTypeBuffer || "message";
      this.#dataBuffer = "";
      this.#eventTypeBuffer = "";
      if (this.#readyState === EventSource.CLOSED) return;
      this.dispatchEvent(new MessageEvent(type, {
        data,
        origin: this.#origin,
        lastEventId: this.#lastEventId,
      }));
    }
  }

  const proto = EventSource.prototype;
  for (const name of ["open", "message", "error"]) {
    natives.defineEventHandler(proto, `on${name}`);
  }
  Object.defineProperty(proto, Symbol.toStringTag, {
    value: "EventSource",
    configurable: true,
  });

  return { ...api, EventSource };
})
