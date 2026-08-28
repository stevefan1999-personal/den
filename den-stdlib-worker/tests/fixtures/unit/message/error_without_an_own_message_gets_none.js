(() => {
  const out = structuredClone(new Error());
  return !Object.hasOwn(out, "message") && out.message === "";
})()
