// den testharnessreport.js: record results on globalThis.__denWpt for the Rust walker.
// Must not create `document` — testharness.js would then pick WindowTestEnvironment
// and wait for a DOM `load` event that never arrives.

if (
  typeof globalThis.location === "undefined" || !globalThis.location.protocol
) {
  globalThis.location = {
    href: "http://127.0.0.1/",
    protocol: "http:",
    host: "127.0.0.1",
    hostname: "127.0.0.1",
    port: "",
    pathname: "/",
    search: "",
    hash: "",
    origin: "http://127.0.0.1",
  };
}

if (typeof globalThis.escape !== "function") {
  globalThis.escape = function (value) {
    return encodeURIComponent(String(value)).replace(/[!'()*]/g, function (ch) {
      return "%" + ch.charCodeAt(0).toString(16).toUpperCase();
    });
  };
}

if (typeof globalThis.GLOBAL === "undefined") {
  globalThis.GLOBAL = {
    isWindow: function () {
      return typeof globalThis.document !== "undefined";
    },
    isWorker: function () {
      return typeof globalThis.document === "undefined";
    },
    isShadowRealm: function () {
      return false;
    },
  };
}

setup({ output: false, explicit_timeout: true, explicit_done: true });

globalThis.__denWpt = { done: false, harness: 0, rows: [] };

add_test_state_callback(function (entry) {
  globalThis.__denWpt.rows[entry.index] = entry;
});

add_completion_callback(function (tests, harness_status) {
  globalThis.__denWpt.harness = harness_status.status;
  globalThis.__denWpt.rows = tests.map(function (entry) {
    return {
      name: entry.name,
      status: entry.status,
      message: entry.message ? String(entry.message) : "",
    };
  });
  globalThis.__denWpt.done = true;
});

globalThis.__denWptEncode = function __denWptEncode(timedOut) {
  return JSON.stringify([
    timedOut,
    globalThis.__denWpt.harness,
    globalThis.__denWpt.rows.filter(Boolean).map(function (row) {
      return [row.status, String(row.name), String(row.message)];
    }),
  ]);
};

globalThis.__denWptWait = async function __denWptWait(ms) {
  var deadline = Date.now() + ms;
  while (!globalThis.__denWpt.done) {
    if (Date.now() > deadline) {
      return globalThis.__denWptEncode(true);
    }
    await new Promise(function (resolve) {
      setTimeout(resolve, 15);
    });
  }
  return globalThis.__denWptEncode(false);
};
