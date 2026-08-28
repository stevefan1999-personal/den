if (typeof self === "undefined") { globalThis.self = globalThis; }
globalThis.window = globalThis;
(function () {
  var nativeSet = globalThis.setTimeout;
  var nativeClear = globalThis.clearTimeout;
  if (typeof nativeSet === "function") {
    globalThis.setTimeout = function (fn, ms) {
      return nativeSet(fn, ms == null ? 0 : Number(ms));
    };
  }
  if (typeof nativeClear === "function") {
    globalThis.clearTimeout = function (id) {
      if (id == null) { return; }
      return nativeClear(id);
    };
  }
})();
