const { assertEquals } = await import("den:assert");
let fired = false;
const pending = setTimeout(() => {
  fired = true;
}, 50);
clearTimeout(pending);
await new Promise((resolve) => setTimeout(resolve, 1));
assertEquals(fired, false);
