// DOM AbortController / AbortSignal. AbortSignal extends EventTarget, which
// is why this lives in the prelude rather than as an `#[rquickjs::class]`.
// Behaviour follows txiki.js's abort-controller polyfill, against den's
// EventTarget (not txiki's copy).
(function (natives, api) {
  const { Event, EventTarget, __defineEventHandler } = api;

  const defaultAbortReason = () =>
    new DOMException("This operation was aborted", "AbortError");

  // Module-private invoker, captured from inside the class so AbortController
  // can flip a signal without exposing `#signalAbort`.
  let signalAbort;

  class AbortSignal extends EventTarget {
    #aborted = false;
    #reason;

    get aborted() {
      return this.#aborted;
    }

    get reason() {
      return this.#reason;
    }

    throwIfAborted() {
      if (this.#aborted) throw this.#reason;
    }

    // Flip to aborted. Returns true if this call made the transition, false
    // if the signal was already aborted (so a second abort is a no-op).
    #signalAbort(reason, dispatch = true) {
      if (this.#aborted) return false;
      this.#aborted = true;
      this.#reason = reason !== undefined ? reason : defaultAbortReason();
      if (dispatch) this.dispatchEvent(new Event("abort"));
      return true;
    }

    static {
      signalAbort = (signal, reason, dispatch) =>
        signal.#signalAbort(reason, dispatch);
    }

    static abort(reason) {
      const signal = new AbortSignal();
      signal.#signalAbort(reason, false);
      return signal;
    }

    // Uses the realm's `setTimeout`. Engine installs den-stdlib-timer as a
    // global; crate tests that only have a sync Context mock it or skip.
    static timeout(ms) {
      const signal = new AbortSignal();
      setTimeout(() => {
        signal.#signalAbort(
          new DOMException(
            "The operation was aborted due to timeout",
            "TimeoutError",
          ),
        );
      }, ms);
      return signal;
    }

    static any(signals) {
      const signal = new AbortSignal();
      for (const source of signals) {
        if (source.aborted) {
          signal.#signalAbort(source.reason, false);
          return signal;
        }
      }
      const onAbort = () => {
        for (const source of signals) {
          if (source.aborted) {
            if (signal.#signalAbort(source.reason)) {
              for (const other of signals) {
                other.removeEventListener("abort", onAbort);
              }
            }
            break;
          }
        }
      };
      for (const source of signals) {
        source.addEventListener("abort", onAbort);
      }
      return signal;
    }
  }

  __defineEventHandler(AbortSignal.prototype, "onabort");
  Object.defineProperty(AbortSignal.prototype, Symbol.toStringTag, {
    value: "AbortSignal",
    configurable: true,
  });

  class AbortController {
    constructor() {
      this.signal = new AbortSignal();
    }

    abort(reason) {
      signalAbort(this.signal, reason);
    }
  }

  Object.defineProperty(AbortController.prototype, Symbol.toStringTag, {
    value: "AbortController",
    configurable: true,
  });

  return { ...api, AbortController, AbortSignal };
})
