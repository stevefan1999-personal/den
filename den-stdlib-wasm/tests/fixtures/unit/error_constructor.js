(() => {
  const C = WebAssembly.$NAME;
  const constructed = new C("boom");
  const called = C("boom");
  return C.length === 1
    && C.name === "$NAME"
    && Object.getPrototypeOf(C) === Error
    && Object.getPrototypeOf(C.prototype) === Error.prototype
    && C.prototype.name === "$NAME"
    && C.prototype.message === ""
    && C.prototype.constructor === C
    && constructed instanceof C
    && constructed instanceof Error
    && constructed.name === "$NAME"
    && constructed.message === "boom"
    && typeof constructed.stack === "string"
    && called instanceof C
    && new C().message === ""
    && new C("x", { cause: 1 }) instanceof C;
})()
