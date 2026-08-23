// `Worker` (HTML §10.2.6) and the dedicated worker global scope (§10.2.1,
// §10.3.1), JS half: the two event targets, the postMessage overloads and the
// error chain's dispatch steps. The thread, the script loading and the
// (de)serialisation are `natives` — see src/worker.rs.
//
// See docs/research/08-web-workers-spec.md §2.7-2.8 and §2.11.
(function (natives, api) {
  const {
    ErrorEvent,
    EventTarget,
    MessageEvent,
    __defineEventHandler,
    __trackMessageListeners,
  } = api;

  // Both overloads of every postMessage here: `(message, [transfer])` and
  // `(message, { transfer })`.
  const splitTransfer = (options) =>
    natives.splitTransfer(Array.isArray(options) ? options : options?.transfer);

  // HTML §8.1.4.6 "report an exception" step 3: fire a cancelable `error` at
  // the global (or, for the parent half of the chain, at the `Worker`), and
  // tell the caller whether anything claimed it. `dispatchEvent` returns false
  // exactly when `preventDefault()` was called.
  const reportErrorAt = (target, message, filename, lineno, colno) =>
    !natives.dispatchTrusted(
      target,
      new ErrorEvent("error", {
        message,
        filename,
        lineno,
        colno,
        cancelable: true,
      }),
    );

  // Where an error that nothing claimed goes next. In the main realm there is
  // nowhere above to escalate to, so it is printed; `installWorkerScope`
  // replaces this with the worker's own half of the chain — its `onerror`, and
  // then its parent — which is what makes an error climb a whole worker tree.
  let escalate = (message, filename, lineno, colno) =>
    natives.reportException(
      `${message}\n    at ${filename}:${lineno}:${colno}`,
    );

  // The `Worker`'s two natives. Symbol-keyed so structured clone drops them
  // and no script can reach the raw channel.
  const PORT = Symbol("den:worker-port");
  const THREAD = Symbol("den:worker-thread");

  class Worker extends EventTarget {
    constructor(scriptURL, options = {}) {
      if (arguments.length < 1) {
        throw new TypeError("Worker constructor: at least 1 argument required");
      }
      // WebIDL enum conversion happens before anything is spawned, so a typo
      // in `type` costs nothing.
      const type = options?.type === undefined ? "classic" : String(options.type);
      if (type !== "classic" && type !== "module") {
        throw new TypeError(
          `Worker constructor: '${type}' is not a valid worker type`,
        );
      }
      const name = options?.name === undefined ? "" : String(options.name);
      const url = String(scriptURL);
      super();

      const [outside, inside] = natives.pair();
      // §10.2.6.1 step 4: the outside port's queue is enabled from here, so a
      // message the worker posts before this realm attaches a listener is
      // queued rather than lost — it is delivered by the pump the first
      // listener starts (see `__trackMessageListeners`). What keeps this realm
      // alive meanwhile is the worker thread itself, not this port.
      const armOutside = __trackMessageListeners(this, outside);
      armOutside();
      const thread = natives.spawn(
        url,
        type,
        name,
        inside,
        // §8.1.4.6 step 4, the parent half: an error the worker's own global
        // did not cancel. `error` is deliberately absent — an Error does not
        // survive the thread boundary (docs/research/10 §4.5).
        (message, filename, lineno, colno) => {
          if (!reportErrorAt(this, message, filename, lineno, colno)) {
            escalate(message, filename, lineno, colno);
          }
        },
      );
      Object.defineProperty(this, PORT, { value: outside });
      Object.defineProperty(this, THREAD, { value: thread });
    }

    postMessage(message, options) {
      const { buffers, ports } = splitTransfer(options);
      this[PORT].post(message, buffers, ports);
    }

    terminate() {
      this[THREAD].terminate();
      // §10.2.4 step 4: everything the worker posted and this realm has not
      // dispatched yet goes away with the port. Idempotent, like the native.
      this[PORT].close();
    }
  }

  __defineEventHandler(Worker.prototype, "onmessage");
  __defineEventHandler(Worker.prototype, "onmessageerror");
  // Ordinary `EventHandler`; only the *global's* `onerror` is the five-argument
  // deviant (HTML §8.1.8.1).
  __defineEventHandler(Worker.prototype, "onerror");

  Object.defineProperty(Worker.prototype, Symbol.toStringTag, {
    value: "Worker",
    configurable: true,
  });

  // The chain a worker global sits on. Built once per realm — a realm has at
  // most one worker global — and only so that `self instanceof EventTarget`
  // and `Object.prototype.toString.call(self)` say what the spec says.
  class WorkerGlobalScope extends EventTarget {}
  class DedicatedWorkerGlobalScope extends WorkerGlobalScope {}
  for (const scopeClass of [WorkerGlobalScope, DedicatedWorkerGlobalScope]) {
    Object.defineProperty(scopeClass.prototype, Symbol.toStringTag, {
      value: scopeClass.name,
      configurable: true,
    });
  }

  // Called from src/worker.rs on the worker's own thread, after the engine is
  // built and before its script runs. Returns the two hooks the thread body
  // needs back: enabling the message queue, and the worker half of the error
  // chain.
  const installWorkerScope = (scope, native, name, hooks) => {
    Object.setPrototypeOf(scope, DedicatedWorkerGlobalScope.prototype);
    let closing = false;

    Object.defineProperties(scope, {
      // §10.2.1: `self` is readonly, `name` is [Replaceable] — which is
      // precisely a writable data property.
      self: { value: scope, configurable: true },
      name: { value: name, writable: true, configurable: true },
      postMessage: {
        value: (message, options) => {
          const { buffers, ports } = splitTransfer(options);
          native.post(message, buffers, ports);
        },
        writable: true,
        configurable: true,
      },
      close: {
        // §10.2.1.2 "close a worker": the closing flag stops *new* work; the
        // current task runs to its end, so `postMessage(result); close();`
        // still delivers. The thread body only observes the cancelled token
        // once this task and its microtasks are done.
        value: () => {
          closing = true;
          hooks.close();
        },
        writable: true,
        configurable: true,
      },
      importScripts: {
        value: (...urls) => hooks.importScripts(urls.map(String)),
        writable: true,
        configurable: true,
      },
      // An unqualified `addEventListener(…)` in a *module* worker resolves
      // through the global environment record, whose this-value is undefined
      // under strict mode; WebIDL substitutes the global there, and an own
      // bound copy is how that substitution is spelled without touching
      // EventTarget itself.
      ...Object.fromEntries(
        ["addEventListener", "removeEventListener", "dispatchEvent"].map((
          method,
        ) => [method, {
          value: EventTarget.prototype[method].bind(scope),
          writable: true,
          configurable: true,
        }]),
      ),
    });

    // Installed before the script runs, so that a listener the script adds on
    // its first line is counted; armed only afterwards, which is what §10.2.4
    // step 2.13 asks for.
    const armInside = __trackMessageListeners(scope, native);

    __defineEventHandler(scope, "onmessage");
    __defineEventHandler(scope, "onmessageerror");
    // The deviant one: `(message, filename, lineno, colno, error)`, cancelled
    // by returning `true` (HTML §8.1.8.1).
    __defineEventHandler(scope, "onerror", true);
    // HTML §8.1.7.5, fired by den-core's rejection tracker at this global
    // (`Engine::fire_rejection_event`); a worker scope is the only global in
    // den that is an EventTarget, so it is the only one that can hear them.
    __defineEventHandler(scope, "onunhandledrejection");
    __defineEventHandler(scope, "onrejectionhandled");

    // HTML §8.1.4.6 "report an exception", worker half: the worker's own
    // global hears about it first, and only what nothing there cancelled
    // travels to the parent. `hooks.fault` is that journey (src/worker.rs).
    escalate = (message, filename, lineno, colno) => {
      if (!reportErrorAt(scope, message, filename, lineno, colno)) {
        hooks.fault(message, filename, lineno, colno);
      }
    };

    // DOM §2.9 inner-invoke step 2.10 says a listener that throws is
    // *reported*, and in a worker realm reporting means the chain above — so a
    // `throw` inside `onmessage` reaches the parent exactly like a `throw` at
    // the script's top level. events.js looks this up on `natives` at call
    // time, which is what makes replacing it here work.
    const print = natives.reportException;
    let reporting = false;
    natives.reportException = (value) => {
      // An exception thrown *while* reporting one — a throwing `onerror` — is
      // printed rather than reported, or the chain would never terminate.
      if (reporting) return print(value);
      reporting = true;
      try {
        const { message, filename, lineno, colno } = hooks.locate(value);
        escalate(message, filename, lineno, colno);
      } finally {
        reporting = false;
      }
    };

    return {
      // §10.2.4 step 2.13: the inside port's queue opens only after the script
      // has run — which is what lets a script assign `onmessage` at its very
      // end and still receive what the parent posted before it existed. A
      // script that called `close()` never opens it at all.
      start: () => {
        if (!closing) armInside();
      },
      reportError: (message, filename, lineno, colno) =>
        reportErrorAt(scope, message, filename, lineno, colno),
    };
  };

  // The installer has to survive until the worker thread calls it, which is
  // long after this prelude's scope is gone; Rust keeps it in the context
  // userdata (src/worker.rs).
  natives.registerScope(installWorkerScope);

  return { ...api, Worker };
})
