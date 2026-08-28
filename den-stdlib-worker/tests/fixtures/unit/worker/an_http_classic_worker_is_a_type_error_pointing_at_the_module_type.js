const failure = (build) => { try { build(); return "no throw"; } catch (error) { return `${error.constructor.name}` } };
[
  failure(() => new Worker("https://example.com/w.js")),
  failure(() => new Worker("./echo.js", { type: "worklet" })),
].join("|")
