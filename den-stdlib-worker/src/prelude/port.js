// MessagePort / MessageChannel (HTML §9.4.2-§9.4.4), JS half: the event
// target, the transfer-list overloads and the queue-enabling rules. The
// channel, the pump and (de)serialisation are `natives` — see src/port.rs.
(function (natives, api) {
  // The events prelude owns all three, and lib.rs evaluates it first; a
  // missing one is a broken build, so let the destructuring throw rather than
  // silently shipping a `den:worker` without MessagePort.
  const { Event, EventTarget, MessageEvent, __defineEventHandler } = api;

  // Where every MessagePort keeps its NativePort: symbol-keyed, so structured
  // clone drops it, and the key the clone pre-pass looks under (clone.js).
  const PORT_HANDLE = natives.portHandleKey;
  // MessagePort has no constructor in the IDL; ports come from a
  // MessageChannel, a transfer, or a Worker. The brand is how those three get
  // in and script does not.
  const BRAND = Symbol("den:port-brand");
  const wrappers = new WeakMap();

  // Enable a port's message queue with the two events it can produce. The
  // ports a message carries arrive as NativePorts and are wrapped by the
  // memoised wrapper, so `event.data`'s port and `event.ports[i]` are the same
  // object (HTML §9.4.4). `afterDispatch` is how the listener tracker below
  // learns that a dispatch has just retired every `once` listener of that type.
  const dispatchMessagesAt = (target, native, afterDispatch = () => {}) =>
    native.start(
      (data, ports) => {
        natives.dispatchTrusted(
          target,
          new MessageEvent("message", { data, ports: ports.map(wrapPort) }),
        );
        afterDispatch("message");
      },
      // A message that could not be deserialised carries nothing: HTML says a
      // `messageerror` MessageEvent, whose data defaults to null.
      () => {
        natives.dispatchTrusted(target, new MessageEvent("messageerror"));
        afterDispatch("messageerror");
      },
      // HTML §9.4.4: the peer went away, so this port is disentangled and
      // nothing else will ever arrive. Only a *running* pump can observe that,
      // so — unlike the spec, which queues the event until the queue is
      // enabled — a port that was never started never hears it.
      // ponytail: the ceiling is the pump; queueing it would mean a second
      // wake-up path that exists only for this one event.
      () => natives.dispatchTrusted(target, new Event("close")),
    );

  // The two events an inbound envelope can become: a target listening for
  // either can still observe what its peer sends, one listening for neither
  // cannot.
  const MESSAGE_EVENTS = ["message", "messageerror"];

  // DOM §2.7 "flatten more", the two members that decide listener identity and
  // lifetime. Kept in step with events.js by hand: it owns the real records,
  // this is only the mirror the ref rule counts.
  const flatten = (options) =>
    typeof options === "boolean"
      ? { capture: options, once: false, signal: undefined }
      : {
          capture: !!options?.capture,
          once: !!options?.once,
          signal: options?.signal ?? undefined,
        };

  // Ref-on-listener — den's process-lifetime rule for ports
  // (docs/research/11 §2.1 rule 2).
  //
  // A message queue the *script* opened keeps its event loop alive until the
  // script closes it: `port.start()`, `onmessage` on a MessagePort, and
  // `new BroadcastChannel` all say "keep listening", and `close()` is how that
  // is taken back. A queue the *platform* opened on the script's behalf — a
  // Worker's two ends, which nobody ever asked to start — keeps the loop alive
  // only while the script can still see what arrives, i.e. while at least one
  // `message` or `messageerror` listener exists. Without that rule
  // `new Worker("noop.js")` hangs den for ever, because the worker's own end is
  // listening to a script that stopped caring.
  //
  // Unreffing stops delivery, never receipt: envelopes stay queued in the
  // channel and the pump resumes at the first one when a listener reappears.
  //
  // Returns the "arm" function, because the two platform queues open at
  // different moments: the parent's end at `new Worker` (§10.2.6.1 step 4) and
  // the worker's own end only once its script has run (§10.2.4 step 2.13).
  const trackMessageListeners = (target, native) => {
    // Mirrors the listener records events.js keeps for the two message types,
    // one bucket per (type, capture) because that pair plus the callback is
    // listener identity. The value is the record's `once` flag: such a record
    // is retired by the dispatch that runs it rather than by a removal.
    const buckets = new Map();
    const bucketFor = (type, capture) => {
      const key = `${type}|${capture}`;
      const bucket = buckets.get(key) ?? new Map();
      buckets.set(key, bucket);
      return bucket;
    };

    let armed = false;
    let refed = false;
    const refresh = () => {
      const listening = armed &&
        [...buckets.values()].some((bucket) => bucket.size > 0);
      if (listening === refed) return;
      refed = listening;
      if (listening) dispatchMessagesAt(target, native, retire);
      else native.pause();
    };
    const retire = (type) => {
      for (const capture of [false, true]) {
        const bucket = bucketFor(type, capture);
        for (const [callback, once] of bucket) {
          if (once) bucket.delete(callback);
        }
      }
      refresh();
    };

    // The originals are reached through `target` rather than the prototype:
    // the worker global is an EventTarget by borrowing these methods, not by
    // being constructed as one.
    const inherited = {
      add: target.addEventListener.bind(target),
      remove: target.removeEventListener.bind(target),
    };
    const track = (type, callback, options) => {
      const { capture, once, signal } = flatten(options);
      // Everything events.js would have refused to record: a null callback, an
      // already-aborted signal, and a duplicate (type, callback, capture).
      if (callback === null || callback === undefined || signal?.aborted) return;
      const bucket = bucketFor(type, capture);
      if (bucket.has(callback)) return;
      bucket.set(callback, once);
      // events.js removes an aborted listener's record without telling anyone,
      // so the mirror has to watch the signal too, or the port would stay
      // reffed for a listener that is gone.
      signal?.addEventListener("abort", () => {
        bucket.delete(callback);
        refresh();
      }, { once: true });
    };
    const property = (value) => ({ value, writable: true, configurable: true });
    Object.defineProperties(target, {
      // Rest arguments throughout: both methods reject a call with fewer than
      // two of them, and delegating first means that check — and every other
      // one events.js makes — still happens before anything is mirrored.
      addEventListener: property((...args) => {
        inherited.add(...args);
        const [type, callback, options] = args;
        if (!MESSAGE_EVENTS.includes(String(type))) return;
        track(String(type), callback, options);
        refresh();
      }),
      removeEventListener: property((...args) => {
        inherited.remove(...args);
        const [type, callback, options] = args;
        if (!MESSAGE_EVENTS.includes(String(type))) return;
        bucketFor(String(type), flatten(options).capture).delete(callback);
        refresh();
      }),
    });

    return () => {
      armed = true;
      refresh();
    };
  };

  class MessagePort extends EventTarget {
    constructor(brand, native) {
      if (brand !== BRAND) throw new TypeError("Illegal constructor");
      super();
      Object.defineProperty(this, PORT_HANDLE, { value: native });
    }

    postMessage(message, options) {
      // Both overloads: `postMessage(m, [buffer])` and `postMessage(m, { transfer })`.
      const transfer = Array.isArray(options) ? options : options?.transfer;
      const { buffers, ports } = natives.splitTransfer(transfer);
      this[PORT_HANDLE].post(message, buffers, ports);
    }

    start() {
      dispatchMessagesAt(this, this[PORT_HANDLE]);
    }

    close() {
      this[PORT_HANDLE].close();
    }
  }

  __defineEventHandler(MessagePort.prototype, "onmessage");
  __defineEventHandler(MessagePort.prototype, "onmessageerror");
  __defineEventHandler(MessagePort.prototype, "onclose");

  // §9.4.4, last paragraph: setting `onmessage` enables the port's message
  // queue "as if the start() method had been called". `addEventListener`
  // deliberately does not — that is the one difference between the two, and
  // the reason a port that only ever gets a listener stays silent.
  const onmessage = Object.getOwnPropertyDescriptor(MessagePort.prototype, "onmessage");
  Object.defineProperty(MessagePort.prototype, "onmessage", {
    ...onmessage,
    set(value) {
      onmessage.set.call(this, value);
      this.start();
    },
  });

  // Memoised, and it has to be: `restore` splices the wrapper into
  // `event.data` while Rust hands the same NativePort back for `event.ports`,
  // and the spec's identity ("the same port object") is what a script tests.
  const wrapPort = (native) => {
    const known = wrappers.get(native);
    if (known !== undefined) return known;
    const port = new MessagePort(BRAND, native);
    wrappers.set(native, port);
    return port;
  };
  // Read by the clone pre-pass, which was evaluated before this file.
  natives.wrapPort = wrapPort;

  Object.defineProperty(MessagePort.prototype, Symbol.toStringTag, {
    value: "MessagePort",
    configurable: true,
  });

  class MessageChannel {
    constructor() {
      const [first, second] = natives.pair();
      Object.defineProperty(this, "port1", { value: wrapPort(first), enumerable: true });
      Object.defineProperty(this, "port2", { value: wrapPort(second), enumerable: true });
    }
  }

  Object.defineProperty(MessageChannel.prototype, Symbol.toStringTag, {
    value: "MessageChannel",
    configurable: true,
  });

  // Internal, for the worker prelude's implicit ports. Enumerable for the same
  // reason as `__defineEventHandler`: the later preludes spread `api`, and only
  // enumerable own properties survive that. lib.rs's API list decides what is
  // actually exported, so the underscore is the whole of its privacy.
  return {
    ...api,
    MessageChannel,
    MessagePort,
    __trackMessageListeners: trackMessageListeners,
    __wrapPort: wrapPort,
  };
})
