// BroadcastChannel (HTML §9.5), JS half: the event target and the closed-state
// rule. The registry, the fan-out and (de)serialisation are `natives` — see
// src/broadcast.rs.
(function (natives, api) {
  // The events prelude owns all three; a missing one is a broken build. See
  // port.js — no guard, so the failure is loud.
  const { EventTarget, MessageEvent, __defineEventHandler } = api;

  const NATIVE = Symbol("den:broadcast-handle");

  class BroadcastChannel extends EventTarget {
    #name;
    #closed = false;

    constructor(name) {
      super();
      this.#name = `${name}`;
      Object.defineProperty(this, NATIVE, { value: new natives.NativeBroadcast(this.#name) });
      // Unlike a MessagePort a BroadcastChannel has no start(): it is
      // "eligible for messages" from the moment it exists, so the pump runs
      // from here and only close() ends it.
      this[NATIVE].subscribe(
        (data) =>
          natives.dispatchTrusted(this, new MessageEvent("message", { data })),
        // A message that could not be deserialised carries nothing: HTML says
        // a `messageerror` MessageEvent, whose data defaults to null.
        () => natives.dispatchTrusted(this, new MessageEvent("messageerror")),
      );
    }

    get name() {
      return this.#name;
    }

    postMessage(message) {
      // §9.5 postMessage step 1: a closed channel is an error, not a no-op —
      // the one place BroadcastChannel differs from MessagePort.
      if (this.#closed) {
        throw new DOMException("the BroadcastChannel is closed", "InvalidStateError");
      }
      this[NATIVE].post(message);
    }

    close() {
      this.#closed = true;
      this[NATIVE].close();
    }
  }

  __defineEventHandler(BroadcastChannel.prototype, "onmessage");
  __defineEventHandler(BroadcastChannel.prototype, "onmessageerror");
  Object.defineProperty(BroadcastChannel.prototype, Symbol.toStringTag, {
    value: "BroadcastChannel",
    configurable: true,
  });

  return { ...api, BroadcastChannel };
})
