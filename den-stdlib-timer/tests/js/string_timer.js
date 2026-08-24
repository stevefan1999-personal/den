const { assert } = await import("den:assert");
setTimeout("globalThis.ran = true", 1);
await new Promise((resolve) => setTimeout(resolve, 20));
assert(globalThis.ran === true);
