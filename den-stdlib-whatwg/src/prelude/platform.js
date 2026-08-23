// Shared platform bits for the later preludes: ProgressEvent / CloseEvent,
// and the onX handler slot used by FileReader, XHR, EventSource and WebSocket.
// Event / EventTarget come from den:worker; they are looked up at evaluate
// time so this crate does not have to own a second copy.
(function (natives, api) {
  const Event = globalThis.Event;
  const EventTarget = globalThis.EventTarget;
  if (typeof Event !== "function" || typeof EventTarget !== "function") {
    throw new TypeError("den:whatwg requires EventTarget (load den:worker first)");
  }

  class ProgressEvent extends Event {
    constructor(type, options = {}) {
      super(type, options);
      this.lengthComputable = !!options.lengthComputable;
      this.loaded = Number(options.loaded) || 0;
      this.total = Number(options.total) || 0;
    }
  }

  class CloseEvent extends Event {
    constructor(type, options = {}) {
      super(type, options);
      this.code = options.code === undefined ? 0 : Number(options.code) || 0;
      this.reason = options.reason === undefined ? "" : String(options.reason);
      this.wasClean = !!options.wasClean;
    }
  }

  const HANDLERS = new WeakMap();
  const handlerFor = (target, name) => {
    let handlers = HANDLERS.get(target);
    if (handlers === undefined) {
      handlers = new Map();
      HANDLERS.set(target, handlers);
    }
    let handler = handlers.get(name);
    if (handler === undefined) {
      handler = { value: null, listener: null };
      handlers.set(name, handler);
    }
    return handler;
  };

  // HTML §8.1.8.1, copied in miniature from den:worker's events.js: the
  // handler occupies the listener-list position of its first non-null
  // assignment. den:worker does not export __defineEventHandler, so each
  // EventTarget subclass in this crate goes through this copy.
  const defineEventHandler = (target, name) => {
    const type = name.slice("on".length);
    Object.defineProperty(target, name, {
      configurable: true,
      enumerable: true,
      get() {
        return handlerFor(this, name).value;
      },
      set(value) {
        const stored = typeof value === "function" ? value : null;
        const handler = handlerFor(this, name);
        handler.value = stored;
        if (stored === null) {
          if (handler.listener !== null) {
            this.removeEventListener(type, handler.listener);
            handler.listener = null;
          }
          return;
        }
        if (handler.listener !== null) return;
        handler.listener = (event) => {
          const callback = handlerFor(this, name).value;
          if (typeof callback === "function") callback.call(this, event);
        };
        this.addEventListener(type, handler.listener);
      },
    });
  };

  for (const platformClass of [ProgressEvent, CloseEvent]) {
    Object.defineProperty(platformClass.prototype, Symbol.toStringTag, {
      value: platformClass.name,
      configurable: true,
    });
  }

  natives.defineEventHandler = defineEventHandler;
  natives.Event = Event;
  natives.EventTarget = EventTarget;

  return { ...api, ProgressEvent, CloseEvent };
})
