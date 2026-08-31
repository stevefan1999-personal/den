import { assert, assertEquals, assertRejects } from "den:assert";
import { Kv } from "den:kv";

const bytes = (...values) => new Uint8Array(values);
const values = (value) => value === null ? null : Array.from(value);
const kv = await Kv.open(__STORE__);
const key = bytes(1);

await kv.set(key, bytes(0));

const committed = await kv.transaction();
await committed.set(key, bytes(1));
assertEquals(values(await committed.get(key)), [1]);
assert(await committed.commit());
assertEquals(values(await kv.get(key)), [1]);
await assertRejects(() => committed.get(key));

const rolledBack = await kv.transaction();
await rolledBack.set(key, bytes(2));
await rolledBack.rollback();
await rolledBack.rollback();
assertEquals(values(await kv.get(key)), [1]);
await assertRejects(() => rolledBack.get(key));

const first = await kv.transaction();
const second = await kv.transaction();
await first.set(key, bytes(3));
await second.set(key, bytes(4));
assert(await first.commit());
assertEquals(await second.commit(), false);
assertEquals(values(await kv.get(key)), [3]);

const checked = await kv.transaction();
assertEquals(values(await checked.getForUpdate(key)), [3]);
const writer = await kv.transaction();
await writer.set(key, bytes(5));
assert(await writer.commit());
await checked.set(bytes(2), bytes(9));
assertEquals(await checked.commit(), false);
assertEquals(await kv.get(bytes(2)), null);

await Promise.all(Array.from({ length: 4 }, (_, value) =>
  kv.set(key, bytes(value))));

const abandoned = await kv.transaction();
await abandoned.set(key, bytes(6));
await kv.close();
await assertRejects(() => abandoned.get(key));
await abandoned.rollback();
