// WebSocket. JS EventTarget wrapping the tokio-tungstenite native.
(function (natives, api) {
  const EventTarget = natives.EventTarget;
  const Event = natives.Event;
  const { CloseEvent } = api;

  class WebSocket extends EventTarget {
    static CONNECTING = 0;
    static OPEN = 1;
    static CLOSING = 2;
    static CLOSED = 3;
    CONNECTING = 0;
    OPEN = 1;
    CLOSING = 2;
    CLOSED = 3;

    #ws;
    #binaryType = "blob";
    #protocol = "";
    #url;
    #readyState;
    #pending = null;

    constructor(url, protocolsOrOptions = []) {
      super();
      let urlStr;
      try {
        urlStr = typeof URL === "function" ? new URL(url).toString() : String(url);
      } catch {
        urlStr = "";
      }
      if (!urlStr || !/^wss?:\/\//i.test(urlStr)) {
        throw new Error("Invalid URL");
      }
      this.#url = urlStr;

      let protocols;
      if (typeof protocolsOrOptions === "string") {
        protocols = [protocolsOrOptions];
      } else if (Array.isArray(protocolsOrOptions)) {
        protocols = protocolsOrOptions;
      } else if (protocolsOrOptions && typeof protocolsOrOptions === "object") {
        protocols = protocolsOrOptions.protocols || [];
      } else {
        protocols = [];
      }
      const protocolStr = protocols.join(",");
      this.#ws = protocolStr
        ? new natives.NativeWebSocket(urlStr, protocolStr)
        : new natives.NativeWebSocket(urlStr);
      this.#readyState = WebSocket.CONNECTING;
      this.#pump();
    }

    #pump() {
      const step = () => {
        this.#pending = this.#ws.nextEvent().then((event) => {
          if (!event) {
            this.#readyState = WebSocket.CLOSED;
            return;
          }
          switch (event.type) {
            case "open":
              this.#protocol = event.protocol || "";
              this.#readyState = WebSocket.OPEN;
              this.dispatchEvent(new Event("open"));
              break;
            case "message": {
              let data = event.data;
              if (event.binary && this.#binaryType === "blob" && typeof Blob === "function") {
                data = new Blob([data]);
              }
              this.dispatchEvent(new MessageEvent("message", { data }));
              break;
            }
            case "error":
              this.dispatchEvent(new ErrorEvent("error", { message: event.message || "" }));
              break;
            case "close":
              this.#readyState = WebSocket.CLOSED;
              this.dispatchEvent(new CloseEvent("close", {
                code: event.code,
                reason: event.reason,
                wasClean: event.code === 1000,
              }));
              return;
            default:
              break;
          }
          if (this.#readyState !== WebSocket.CLOSED) step();
        });
      };
      step();
    }

    get binaryType() {
      return this.#binaryType;
    }
    set binaryType(value) {
      if (!["arraybuffer", "blob"].includes(value)) {
        throw new Error(`Unsupported binaryType: ${value}`);
      }
      this.#binaryType = value;
    }
    get protocol() {
      return this.#protocol;
    }
    get readyState() {
      return this.#readyState;
    }
    get url() {
      return this.#url;
    }
    get bufferedAmount() {
      return 0;
    }
    get extensions() {
      return "";
    }

    send(data) {
      if (this.#readyState === WebSocket.CONNECTING) {
        throw new DOMException("WebSocket is not open", "InvalidStateError");
      }
      if (this.#readyState !== WebSocket.OPEN) return;
      if (typeof data === "string") {
        this.#ws.sendText(data);
      } else if (typeof Blob === "function" && data instanceof Blob) {
        data.arrayBuffer().then((buf) => {
          this.#ws.sendBinary(new Uint8Array(buf));
        });
      } else if (data instanceof ArrayBuffer) {
        this.#ws.sendBinary(new Uint8Array(data));
      } else if (ArrayBuffer.isView(data)) {
        this.#ws.sendBinary(new Uint8Array(data.buffer, data.byteOffset, data.byteLength));
      }
    }

    close(code = 1000, reason = "") {
      if (this.#readyState === WebSocket.CLOSING || this.#readyState === WebSocket.CLOSED) {
        return;
      }
      if (code !== 1000 && !(code >= 3000 && code <= 4999)) {
        throw new RangeError("Invalid code value");
      }
      if (String(reason).length > 123) {
        throw new SyntaxError("Invalid reason value");
      }
      this.#readyState = WebSocket.CLOSING;
      this.#ws.close(code, String(reason));
    }
  }

  const proto = WebSocket.prototype;
  for (const name of ["close", "error", "message", "open"]) {
    natives.defineEventHandler(proto, `on${name}`);
  }
  Object.defineProperty(proto, Symbol.toStringTag, {
    value: "WebSocket",
    configurable: true,
  });

  return { ...api, WebSocket };
})
