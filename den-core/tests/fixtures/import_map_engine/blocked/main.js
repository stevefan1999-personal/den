globalThis.got = await import("blocked").then(
  () => "ok",
  (error) => `threw:${error}`,
);
