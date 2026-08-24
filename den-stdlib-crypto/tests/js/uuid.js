const { assert, assertNotEquals } = await import("den:assert");
const uuid = crypto.randomUUID();
assert(
  /^[0-9a-f]{8}-[0-9a-f]{4}-4[0-9a-f]{3}-[89ab][0-9a-f]{3}-[0-9a-f]{12}$/.test(uuid),
);
assertNotEquals(crypto.randomUUID(), uuid);
