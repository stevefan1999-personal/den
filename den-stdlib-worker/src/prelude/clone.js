// Structured clone, JS half: everything the byte serialiser
// (`JS_WriteObject2`) gets wrong, plus the transfer-list plumbing.
//
// `prepare` runs on the sender before the write, `restore` on the receiver
// after the read; both are handed to Rust through `natives.registerClone`.
// See docs/research/10-structured-clone-strategy.md for what the serialiser
// does and does not carry, with quickjs.c line references.
(function (natives, api) {
  // The tag key marking the four things the serialiser cannot carry. A leading
  // NUL cannot appear in an identifier and essentially never in real data.
  // A sender that puts this key in its own data will see it revived on the far
  // side: that is its own data coming back, not a trust boundary.
  const TAG = "\u0000den:structured-clone";
  // Where a JS `MessagePort` keeps its `NativePort`. Published on `natives`
  // because the port prelude sets it and this pre-pass reads it.
  const PORT_HANDLE = Symbol("den:port-handle");
  // `instanceof` is forgeable and `Object.getPrototypeOf` runs a Proxy trap, so
  // "is this an ordinary object rather than a platform object" is answered by
  // class id — the ids are a private enum in quickjs.c, so a freshly made `{}`
  // is the only way to name JS_CLASS_OBJECT.
  const PLAIN_OBJECT_CLASS = natives.classIdOf({});
  // Spec step 17.2: any other error name degrades to "Error".
  const ERROR_CONSTRUCTORS = new Map([
    ["Error", Error], ["EvalError", EvalError], ["RangeError", RangeError],
    ["ReferenceError", ReferenceError], ["SyntaxError", SyntaxError],
    ["TypeError", TypeError], ["URIError", URIError],
  ]);
  const hasOwn = Object.hasOwn;

  // Every property this file puts on a value it built goes through here, never
  // through assignment: [[Set]] walks the prototype chain, so `out.x = v` hands
  // the data to an `Object.prototype` setter and creates nothing, and an own
  // `__proto__` *data* property — what `JSON.parse('{"__proto__":1}')` yields —
  // is swallowed by the `Object.prototype.__proto__` accessor instead of being
  // copied. The spec builds its output with CreateDataProperty (step 26.4.1.3),
  // which is exactly this descriptor.
  const defineData = (target, key, value) =>
    Object.defineProperty(target, key, {
      value, writable: true, enumerable: true, configurable: true,
    });

  const fail = (what) => {
    throw new DOMException(`${what} could not be cloned.`, "DataCloneError");
  };

  const withStack = (error, stack) => {
    if (typeof stack === "string") {
      Object.defineProperty(error, "stack", {
        value: stack, writable: true, configurable: true,
      });
    }
    return error;
  };

  // Types the serialiser handles natively and identically to the spec: pass
  // them through by reference so that aliasing (two views over one buffer) and
  // identity survive into the rebuilt graph.
  //
  // RegExp is NOT among them, even though `JS_WriteRegExp` carries source and
  // flags perfectly: `JS_ReadRegExp` (quickjs.c:39435) is the one reader that
  // forgets to `BC_add_object_ref` the object it just built, while the writer
  // adds *every* object to its reference list (quickjs.c:38288). So a graph
  // with a RegExp in it shifts every later back-reference by one, and the read
  // fails with "invalid object reference" or revives the wrong object. Same
  // shape of quickjs-ng bug as the Map one below, same workaround: rebuild it
  // from its parts and never emit BC_TAG_REGEXP.
  const isLeaf = (value) =>
    (ArrayBuffer.isView(value) && !(value instanceof DataView))
    || value instanceof ArrayBuffer || value instanceof Date
    || value instanceof Number || value instanceof String
    || value instanceof Boolean || value instanceof BigInt;

  // The intrinsic element read, captured before any script can shadow it on an
  // instance. Every %TypedArray% accessor refuses an out-of-bounds receiver and
  // `at` is the cheapest: no allocation, no mutation, and `undefined` for a
  // view that is merely empty.
  const typedArrayAt = Object.getPrototypeOf(Uint8Array.prototype).at;
  const dataViewByteOffset =
    Object.getOwnPropertyDescriptor(DataView.prototype, "byteOffset").get;

  // The spec's IsArrayBufferViewOutOfBounds: a view whose resizable buffer was
  // shrunk under it — or detached — is not serializable, and the serialisation
  // steps make that a DataCloneError.
  //
  // It has to be caught *here* because quickjs cannot be relied on to say so:
  // it answers `byteOffset` with 0 for such a TypedArray, throws a bare
  // TypeError for such a DataView, and its writer records the stale offset
  // regardless — so the only complaint comes from the *reader*, which across a
  // worker runs on the far side long after the sender returned, as a
  // `messageerror` where HTML wants a synchronous throw.
  const isOutOfBounds = (view) => {
    try {
      if (view instanceof DataView) dataViewByteOffset.call(view);
      else typedArrayAt.call(view, 0);
      return false;
    } catch {
      return true;
    }
  };

  // Types the spec rejects outright (steps 21-23). The serialiser would reject
  // most of them too, but with a `TypeError` and a useless message.
  const forbiddenName = (value) =>
      value instanceof Promise ? "Promise"
    : value instanceof WeakMap ? "WeakMap"
    : value instanceof WeakSet ? "WeakSet"
    : value instanceof WeakRef ? "WeakRef"
    : value instanceof FinalizationRegistry ? "FinalizationRegistry"
    : typeof SharedArrayBuffer === "function" && value instanceof SharedArrayBuffer
      ? "SharedArrayBuffer"
    : undefined;

  // Sender side. `ports` are the NativePorts of the transfer list, in order; a
  // port in the graph is replaced by its index and rebuilt by `restore`.
  const prepare = (value, ports) => {
    const seen = new Map();
    const transferred = new Map(ports.map((port, index) => [port, index]));
    const remember = (from, to) => { seen.set(from, to); return to; };
    const tag = (kind, fields) => ({ [TAG]: kind, ...fields });

    const copy = (value) => {
      switch (typeof value) {
        case "symbol": fail(String(value));
        case "function": fail(`function ${value.name || "(anonymous)"}`);
        case "object": if (value !== null) break;
        default: return value;
      }
      const hit = seen.get(value);
      if (hit !== undefined) return hit; // cycles and shared references
      // Before anything that could run a trap, `instanceof` included.
      if (natives.isProxy(value)) fail("#<Proxy>");
      const forbidden = forbiddenName(value);
      if (forbidden !== undefined) fail(forbidden);

      const port = value[PORT_HANDLE];
      if (port !== undefined) {
        // MessagePort is [Transferable] but not [Serializable] (HTML §9.4.4).
        const index = transferred.get(port);
        if (index === undefined) fail("a MessagePort that is not in the transfer list");
        return remember(value, tag("Port", { index }));
      }
      if (ArrayBuffer.isView(value) && isOutOfBounds(value)) {
        fail("an ArrayBufferView over a detached or resized buffer");
      }
      if (isLeaf(value)) return remember(value, value);
      if (value instanceof RegExp) {
        // `lastIndex` is deliberately not carried: the spec's RegExp
        // deserialisation (step 17.5) builds a fresh object, whose lastIndex is
        // 0 whatever the source's was.
        return remember(value, tag("RegExp", { source: value.source, flags: value.flags }));
      }
      if (value instanceof DataView) {
        return remember(value, tag("DataView", {
          buffer: value.buffer, byteOffset: value.byteOffset, byteLength: value.byteLength,
        }));
      }
      // DOMException has its own class id, so `natives.isError` is false for it
      // and it has to be checked first.
      if (value instanceof DOMException) {
        return remember(value, tag("DOMException", {
          name: value.name, message: value.message,
          stack: typeof value.stack === "string" ? value.stack : undefined,
        }));
      }
      if (natives.isError(value)) {
        const out = remember(value, tag("Error", {
          name: ERROR_CONSTRUCTORS.has(value.name) ? value.name : "Error",
          message: hasOwn(value, "message") ? String(value.message) : undefined,
          stack: typeof value.stack === "string" ? value.stack : undefined,
        }));
        // `cause` is not required by step 17, but it costs one recursion and a
        // silently dropped cause is what wastes the afternoon.
        if (hasOwn(value, "cause")) defineData(out, "cause", copy(value.cause));
        return out;
      }
      if (Array.isArray(value)) {
        // Holes stay holes here; the serialiser fills them with `undefined`,
        // and non-index properties are dropped. Both are deliberate v1
        // divergences (docs/research/10 §1.3), pinned by tests.
        return copyOwn(value, remember(value, new Array(value.length)));
      }
      // Map/Set are REBUILT rather than passed through: `js_map_write`
      // (quickjs.c:52802) emits zombie records that `js_map_read` does not
      // expect, desynchronising the whole stream — docs/research/10 §4.4.
      if (value instanceof Map) {
        const out = remember(value, new Map());
        for (const [key, item] of value) out.set(copy(key), copy(item));
        return out;
      }
      if (value instanceof Set) {
        const out = remember(value, new Set());
        for (const item of value) out.add(copy(item));
        return out;
      }
      if (natives.classIdOf(value) === PLAIN_OBJECT_CLASS) {
        // Rebuilding is what invokes getters (spec step 26.4.1.1, which the
        // serialiser refuses outright) and what drops symbol keys (step 26.4
        // yields String keys only).
        return copyOwn(value, remember(value, {}));
      }
      // Platform objects and other exotics: hand them to the serialiser, which
      // refuses them, and the Rust side re-tags that as a DataCloneError.
      return remember(value, value);
    };

    // Spec step 26.4: the key list is taken once, up front, and ownership is
    // re-checked for each entry — a getter is free to `delete` a key that is
    // still in the snapshot, and the deleted key must be omitted rather than
    // resurrected as an own `undefined`.
    const copyOwn = (from, to) => {
      for (const key of Object.keys(from)) {
        if (hasOwn(from, key)) defineData(to, key, copy(from[key]));
      }
      return to;
    };

    return copy(value);
  };

  const wrapPort = (port) => {
    // Installed by the port prelude; absent only in a cut-down `den:worker`.
    if (typeof natives.wrapPort !== "function") fail("MessagePort");
    return natives.wrapPort(port);
  };

  // Receiver side. The graph is freshly deserialised and nobody else holds a
  // reference to it, so containers are revived in place.
  const restore = (value, ports) => {
    const seen = new Map();
    const remember = (from, to) => { seen.set(from, to); return to; };

    const revive = (value) => {
      if (typeof value !== "object" || value === null) return value;
      const hit = seen.get(value);
      if (hit !== undefined) return hit;

      switch (value[TAG]) {
        case "Port": return remember(value, wrapPort(ports[value.index]));
        case "RegExp": return remember(value, new RegExp(value.source, value.flags));
        case "DataView":
          return remember(value, new DataView(value.buffer, value.byteOffset, value.byteLength));
        case "DOMException":
          return withStack(remember(value, new DOMException(value.message, value.name)), value.stack);
        case "Error": {
          const construct = ERROR_CONSTRUCTORS.get(value.name) ?? Error;
          const out = withStack(remember(value, new construct(value.message)), value.stack);
          // After `remember`, so an error that is its own cause terminates.
          if (hasOwn(value, "cause")) defineData(out, "cause", revive(value.cause));
          return out;
        }
        default: break;
      }

      remember(value, value);
      if (Array.isArray(value)) {
        for (const key of Object.keys(value)) value[key] = revive(value[key]);
      } else if (value instanceof Map) {
        const entries = [...value];
        value.clear();
        for (const [key, item] of entries) value.set(revive(key), revive(item));
      } else if (value instanceof Set) {
        const items = [...value];
        value.clear();
        for (const item of items) value.add(revive(item));
      } else if (natives.classIdOf(value) === PLAIN_OBJECT_CLASS) {
        for (const key of Object.keys(value)) value[key] = revive(value[key]);
      }
      return value;
    };

    return revive(value);
  };

  // Split a transfer list into the two kinds Rust can act on. Shared with the
  // port, worker and broadcast preludes so the rules live in one place.
  const splitTransfer = (transfer) => {
    const buffers = [];
    const ports = [];
    if (transfer === undefined || transfer === null) return { buffers, ports };
    for (const entry of transfer) {
      if (entry instanceof ArrayBuffer) { buffers.push(entry); continue; }
      const port = entry?.[PORT_HANDLE];
      if (port !== undefined) { ports.push(port); continue; }
      fail("a value in the transfer list");
    }
    return { buffers, ports };
  };

  const structuredClone = (value, options) => {
    const { buffers, ports } = splitTransfer(options?.transfer);
    return natives.cloneWithTransfer(value, buffers, ports);
  };

  natives.portHandleKey = PORT_HANDLE;
  natives.splitTransfer = splitTransfer;
  natives.registerClone(prepare, restore);

  return { ...api, structuredClone };
})
