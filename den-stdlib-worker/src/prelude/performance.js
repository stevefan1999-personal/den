// High Resolution Time `performance.now` / `timeOrigin`. The clock is a
// native Instant captured when natives install; this is the object a script
// sees, replacing QuickJS-ng's intrinsic of the same name.
(function (natives, api) {
  class Performance {
    now() {
      return natives.now();
    }

    get timeOrigin() {
      return natives.timeOrigin;
    }
  }

  Object.defineProperty(Performance.prototype, Symbol.toStringTag, {
    value: "Performance",
    configurable: true,
  });

  return { ...api, performance: new Performance() };
})
