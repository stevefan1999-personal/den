// DOM §2 `EventTarget`/`Event`, the event subclasses every other prelude
// dispatches, HTML §8.1.8.1's `onX` handler slots and §8.1.4.7's
// `reportError()`.
//
// This is the base of the whole prelude: `MessagePort`, `Worker`,
// `BroadcastChannel` and the worker global all extend or borrow `EventTarget`,
// which is exactly why the API surface is JS-land — an `#[rquickjs::class]`
// cannot extend one.
//
// See docs/research/08-web-workers-spec.md §2.1-2.5 for the spec steps each
// piece implements and the list of members that are N/A in a CLI runtime.
(function (natives, api) {
  // All per-object state is keyed on the object in a WeakMap rather than kept
  // in a private field, because the worker global becomes an EventTarget by
  // *borrowing* these methods — it is never constructed by `EventTarget`, and a
  // private field would brand-check it away.
  const LISTENERS = new WeakMap(); // target -> Map(type -> listener record[])
  const HANDLERS = new WeakMap(); // target -> Map(name -> event handler)
  const EVENT_STATE = new WeakMap(); // event -> its attributes and flags

  // `timeStamp` is relative to a time origin; the moment this realm loaded is
  // the only origin den has (there is no `performance.timeOrigin`).
  const TIME_ORIGIN = Date.now();

  // DOM §2.2. Dispatch here is always AT_TARGET — nothing den exposes lives in
  // a tree — but the constants are observable, so they exist.
  const PHASE = { NONE: 0, CAPTURING_PHASE: 1, AT_TARGET: 2, BUBBLING_PHASE: 3 };

  const stateOf = (event) => {
    const state = EVENT_STATE.get(event);
    if (state === undefined) {
      throw new TypeError("Illegal invocation: not an Event");
    }
    return state;
  };

  const upsert = (weakMap, key) => {
    let entry = weakMap.get(key);
    if (entry === undefined) {
      entry = new Map();
      weakMap.set(key, entry);
    }
    return entry;
  };

  // WebIDL `unsigned long`, which is where `lineno`/`colno` land.
  const toUnsignedLong = (value) => Number(value) >>> 0;
  const toStringOr = (value, fallback) =>
    value === undefined ? fallback : String(value);

  class Event {
    constructor(type, options = {}) {
      if (arguments.length < 1) {
        throw new TypeError("Event constructor: at least 1 argument required");
      }
      // The spec's "initialized" flag is set here and never cleared: den has no
      // `document.createEvent`, so an Event that exists is initialized —
      // `initEvent` can only re-initialize one — and only the dispatch flag can
      // fail `dispatchEvent`.
      EVENT_STATE.set(this, {
        type: String(type),
        target: null,
        currentTarget: null,
        eventPhase: PHASE.NONE,
        bubbles: !!options.bubbles,
        cancelable: !!options.cancelable,
        composed: !!options.composed,
        canceled: false,
        stopPropagation: false,
        stopImmediate: false,
        dispatch: false,
        isTrusted: false,
        timeStamp: Date.now() - TIME_ORIGIN,
      });
    }

    get type() {
      return stateOf(this).type;
    }
    get target() {
      return stateOf(this).target;
    }
    get currentTarget() {
      return stateOf(this).currentTarget;
    }
    get eventPhase() {
      return stateOf(this).eventPhase;
    }
    get bubbles() {
      return stateOf(this).bubbles;
    }
    get cancelable() {
      return stateOf(this).cancelable;
    }
    get composed() {
      return stateOf(this).composed;
    }
    get defaultPrevented() {
      return stateOf(this).canceled;
    }
    get isTrusted() {
      return stateOf(this).isTrusted;
    }
    get timeStamp() {
      return stateOf(this).timeStamp;
    }
    // DOM §2.2 legacy members. `srcElement` aliases `target`; `cancelBubble`
    // and `returnValue` are the IE-era spellings of the two flags, each of
    // which only ever moves in one direction.
    get srcElement() {
      return stateOf(this).target;
    }
    get cancelBubble() {
      return stateOf(this).stopPropagation;
    }
    set cancelBubble(value) {
      if (value) stateOf(this).stopPropagation = true;
    }
    get returnValue() {
      return !stateOf(this).canceled;
    }
    set returnValue(value) {
      if (!value) this.preventDefault();
    }

    // The path is built during dispatch and emptied when it ends, so this is
    // `[]` outside one. Nothing den exposes lives in a tree, so the path a
    // dispatch does build is the single target.
    composedPath() {
      const state = stateOf(this);
      return state.dispatch ? [state.target] : [];
    }

    // DOM §2.2 "initialize": a no-op mid-dispatch, and otherwise a reset of
    // everything the constructor set except the data the subclass carries.
    initEvent(type, bubbles = false, cancelable = false) {
      if (arguments.length < 1) {
        throw new TypeError("initEvent: at least 1 argument required");
      }
      const state = stateOf(this);
      if (state.dispatch) return;
      state.type = String(type);
      state.bubbles = !!bubbles;
      state.cancelable = !!cancelable;
      state.canceled = false;
      state.stopPropagation = false;
      state.stopImmediate = false;
      state.isTrusted = false;
      state.target = null;
    }

    preventDefault() {
      const state = stateOf(this);
      if (state.cancelable) state.canceled = true;
    }

    // Propagation has nothing to travel through here, but a listener may still
    // call this, and the flag is cleared at the end of dispatch either way.
    stopPropagation() {
      stateOf(this).stopPropagation = true;
    }

    stopImmediatePropagation() {
      const state = stateOf(this);
      state.stopPropagation = true;
      state.stopImmediate = true;
    }
  }

  for (const [name, value] of Object.entries(PHASE)) {
    const constant = { value, enumerable: true };
    Object.defineProperty(Event, name, constant);
    Object.defineProperty(Event.prototype, name, constant);
  }

  // DOM §2.4. The only event type a script can carry arbitrary data on, and
  // the one `dispatchEvent` callers reach for; nothing in den fires it.
  class CustomEvent extends Event {
    constructor(type, options = {}) {
      super(type, options);
      stateOf(this).detail = options.detail ?? null;
    }

    get detail() {
      return stateOf(this).detail;
    }

    initCustomEvent(type, bubbles = false, cancelable = false, detail = null) {
      if (arguments.length < 1) {
        throw new TypeError("initCustomEvent: at least 1 argument required");
      }
      const state = stateOf(this);
      if (state.dispatch) return;
      this.initEvent(type, bubbles, cancelable);
      state.detail = detail;
    }
  }

  class MessageEvent extends Event {
    constructor(type, options = {}) {
      super(type, options);
      const state = stateOf(this);
      state.data = options.data ?? null;
      // `origin` and `lastEventId` exist for postMessage-between-origins and
      // server-sent events; den has neither, so they are always "".
      state.origin = toStringOr(options.origin, "");
      state.lastEventId = toStringOr(options.lastEventId, "");
      state.source = options.source ?? null;
      // HTML §9.1: `ports` is a FrozenArray, and the transfer order it arrives
      // in is observable, so it is copied rather than aliased.
      state.ports = Object.freeze([...(options.ports ?? [])]);
    }

    get data() {
      return stateOf(this).data;
    }
    get origin() {
      return stateOf(this).origin;
    }
    get lastEventId() {
      return stateOf(this).lastEventId;
    }
    get source() {
      return stateOf(this).source;
    }
    get ports() {
      return stateOf(this).ports;
    }
  }

  class ErrorEvent extends Event {
    constructor(type, options = {}) {
      super(type, options);
      const state = stateOf(this);
      state.message = toStringOr(options.message, "");
      state.filename = toStringOr(options.filename, "");
      state.lineno = toUnsignedLong(options.lineno);
      state.colno = toUnsignedLong(options.colno);
      // HTML §8.1.4.6: `error` initializes to undefined, not null.
      state.error = options.error;
    }

    get message() {
      return stateOf(this).message;
    }
    get filename() {
      return stateOf(this).filename;
    }
    get lineno() {
      return stateOf(this).lineno;
    }
    get colno() {
      return stateOf(this).colno;
    }
    get error() {
      return stateOf(this).error;
    }
  }

  // HTML §8.1.7.5. den-core's promise-rejection tracker constructs this and
  // dispatches it at the realm's global (see `Engine::fire_rejection_event`),
  // treating a cancelled dispatch as "the realm handled it". `promise` is a
  // required init member, hence the two-argument minimum.
  class PromiseRejectionEvent extends Event {
    constructor(type, options = {}) {
      if (arguments.length < 2) {
        throw new TypeError(
          "PromiseRejectionEvent constructor: at least 2 arguments required",
        );
      }
      super(type, options);
      const state = stateOf(this);
      state.promise = options.promise;
      state.reason = options.reason;
    }

    get promise() {
      return stateOf(this).promise;
    }
    get reason() {
      return stateOf(this).reason;
    }
  }

  // DOM §2.7 "flatten more". A boolean argument is `capture`; capture itself is
  // N/A — with no tree every dispatch is AT_TARGET, where inner invoke runs
  // capturing and bubbling listeners alike in registration order — but it still
  // participates in listener identity, so it is stored and matched on.
  const flatten = (options) =>
    typeof options === "boolean"
      ? { capture: options, once: false, signal: undefined }
      : {
          capture: !!options?.capture,
          once: !!options?.once,
          signal: options?.signal ?? undefined,
        };

  const removeRecord = (target, record) => {
    // DOM §2.9 inner invoke step 2 iterates listeners "whose removed is false":
    // the flag is what makes a listener removed *during* a dispatch skipped,
    // since the dispatch iterates a clone of the list taken before it started.
    record.removed = true;
    const list = LISTENERS.get(target)?.get(record.type);
    const index = list === undefined ? -1 : list.indexOf(record);
    if (index >= 0) list.splice(index, 1);
  };

  const invokeListener = (record, event, currentTarget) => {
    try {
      if (typeof record.callback === "function") {
        record.callback.call(currentTarget, event);
      } else {
        // "Call a user object's operation": a plain object listener is invoked
        // through `handleEvent`, and a missing one is a TypeError — reported
        // like any other listener exception.
        record.callback.handleEvent(event);
      }
    } catch (error) {
      // DOM §2.9 inner invoke step 2.10: report, then keep going. Swallowing
      // the exception here is the spec's behaviour, not a shortcut.
      natives.reportException(error);
    }
  };

  // DOM §2.7 dispatchEvent step 2 forces `isTrusted` false, which is right for
  // a script dispatch and wrong for the events den itself fires (`message`,
  // `messageerror`, `error`) — per DOM those are trusted. This marks the event
  // on its way in and `dispatchEvent` honours the mark; the only way to reach
  // it is `natives`, the bag no script holds, so trust cannot be forged. A
  // nested dispatch from inside a listener carries a different event and stays
  // untrusted, which is what makes a single marker enough.
  let trustedEvent = null;
  const dispatchTrusted = (target, event) => {
    trustedEvent = event;
    try {
      return target.dispatchEvent(event);
    } finally {
      trustedEvent = null;
    }
  };

  class EventTarget {
    addEventListener(type, callback, options = {}) {
      if (arguments.length < 2) {
        throw new TypeError(
          "addEventListener: at least 2 arguments required",
        );
      }
      if (
        callback !== null && callback !== undefined &&
        typeof callback !== "function" && typeof callback !== "object"
      ) {
        throw new TypeError("addEventListener: callback is not an object");
      }
      const { capture, once, signal } = flatten(options);
      if (signal?.aborted) return;
      // A null callback is not an error, it is a no-op (DOM §2.7 step 3).
      if (callback === null || callback === undefined) return;

      const record = { type: String(type), callback, capture, once, removed: false };
      const listeners = upsert(LISTENERS, this);
      const list = listeners.get(record.type);
      if (list === undefined) {
        listeners.set(record.type, [record]);
      } else {
        // Identity is (type, callback, capture); a duplicate is dropped rather
        // than appended, so the same handler never fires twice.
        const duplicate = list.some((other) =>
          other.callback === callback && other.capture === capture
        );
        if (duplicate) return;
        list.push(record);
      }
      // den has no `AbortController` yet, so this is duck-typed rather than
      // brand-checked: whatever ships one will work without touching this file.
      if (signal) {
        signal.addEventListener("abort", () => removeRecord(this, record), {
          once: true,
        });
      }
    }

    removeEventListener(type, callback, options = {}) {
      if (arguments.length < 2) {
        throw new TypeError(
          "removeEventListener: at least 2 arguments required",
        );
      }
      const { capture } = flatten(options);
      const list = LISTENERS.get(this)?.get(String(type));
      const record = list?.find((other) =>
        other.callback === callback && other.capture === capture
      );
      if (record !== undefined) removeRecord(this, record);
    }

    dispatchEvent(event) {
      const state = stateOf(event);
      if (state.dispatch) {
        throw new DOMException(
          "The event is already being dispatched.",
          "InvalidStateError",
        );
      }
      // DOM §2.7 dispatchEvent step 2: a script-dispatched event is untrusted,
      // and only the runtime's own marker says otherwise.
      state.isTrusted = event === trustedEvent;
      state.dispatch = true;
      state.target = this;
      state.currentTarget = this;
      state.eventPhase = PHASE.AT_TARGET;
      // Cloned before iterating (DOM §2.9 step 6.13): a listener added during
      // this dispatch must not be called by it.
      const listeners = [...(LISTENERS.get(this)?.get(state.type) ?? [])];
      for (const record of listeners) {
        if (record.removed) continue;
        // `once` is removed *before* the call, so a handler that re-dispatches
        // does not see it.
        if (record.once) removeRecord(this, record);
        invokeListener(record, event, this);
        if (state.stopImmediate) break;
      }
      state.eventPhase = PHASE.NONE;
      state.currentTarget = null;
      state.dispatch = false;
      state.stopPropagation = false;
      state.stopImmediate = false;
      return !state.canceled;
    }
  }

  // HTML §8.1.4.7. "Report the exception" is precisely what
  // `natives.reportException` already does: print it in the main realm, and in
  // a worker fire the global's own `error` event and escalate what nothing
  // there cancelled to the parent (worker.js replaces the hook to do that), so
  // this is a rename rather than a second implementation. Looked up at call
  // time for the same reason.
  function reportError(value) {
    if (arguments.length < 1) {
      throw new TypeError("reportError: at least 1 argument required");
    }
    natives.reportException(value);
  }

  const handlerFor = (target, name) => {
    const handlers = upsert(HANDLERS, target);
    let handler = handlers.get(name);
    if (handler === undefined) {
      handler = { value: null, listener: null };
      handlers.set(name, handler);
    }
    return handler;
  };

  // HTML §8.1.8.1. The point of the indirection: the handler occupies the
  // position in the listener list of its *first* non-null assignment, so
  // `t.onmessage = a; t.addEventListener("message", b); t.onmessage = c` still
  // runs c before b. Reassignment only swaps the stored value; assigning null
  // deactivates, and the next assignment appends at the end.
  //
  // `globalOnError` selects the one deviant signature in the spec: `onerror` on
  // a global receives (message, filename, lineno, colno, error) and cancels on
  // `true`, where every other handler receives (event) and cancels on `false`.
  const defineEventHandler = (target, name, globalOnError = false) => {
    const type = name.slice("on".length);
    Object.defineProperty(target, name, {
      configurable: true,
      // Non-enumerable to match the class-syntax members above; ARCHITECTURE's
      // convention for den's platform objects (docs/research/08 §2.0).
      enumerable: false,
      get() {
        return handlerFor(this, name).value;
      },
      set(value) {
        // [LegacyTreatNonObjectAsNull]: a non-object (a string, a number)
        // stores null, while a non-callable *object* is stored and simply never
        // called.
        const stored =
          typeof value === "function" ||
            (typeof value === "object" && value !== null)
            ? value
            : null;
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
          // Re-read: the slot may have been reassigned since activation.
          const callback = handlerFor(this, name).value;
          if (typeof callback !== "function") return;
          const special = globalOnError && event instanceof ErrorEvent &&
            event.type === "error";
          const returned = special
            ? callback.call(
              this,
              event.message,
              event.filename,
              event.lineno,
              event.colno,
              event.error,
            )
            : callback.call(this, event);
          if (special ? returned === true : returned === false) {
            event.preventDefault();
          }
        };
        this.addEventListener(type, handler.listener);
      },
    });
  };

  for (
    const platformClass of [
      EventTarget, Event, CustomEvent, MessageEvent, ErrorEvent,
      PromiseRejectionEvent,
    ]
  ) {
    Object.defineProperty(platformClass.prototype, Symbol.toStringTag, {
      value: platformClass.name,
      configurable: true,
    });
  }

  // `__defineEventHandler` is internal plumbing for the later preludes, not
  // part of `den:worker`'s exports (lib.rs's API list decides those), hence the
  // underscore. It must stay *enumerable*: every later prelude carries `api`
  // forward with `{ ...api }`, and a spread copies enumerable own properties
  // only, so a non-enumerable slot would be dropped before port.js sees it.
  // `dispatchTrusted` is deliberately *not* on `api`: `api` becomes den:worker's
  // exports and its globals, and a script that could call this could forge a
  // trusted event. `natives` is the module-private bag, and the preludes that
  // fire runtime events (port.js, worker.js, broadcast.js) are its callers.
  natives.dispatchTrusted = dispatchTrusted;

  return {
    ...api,
    CustomEvent,
    ErrorEvent,
    Event,
    EventTarget,
    MessageEvent,
    PromiseRejectionEvent,
    reportError,
    __defineEventHandler: defineEventHandler,
  };
})
