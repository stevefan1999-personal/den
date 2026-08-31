import { assertEquals, assertRejects } from "den:assert";
import { Kv } from "den:kv";

const bytes = (...values) => new Uint8Array(values);
const path = __STORE__;

await assertRejects(() => Kv.open(""));

const kv = await Kv.open(path);
await assertRejects(() => Kv.open(path));

const key = bytes(0, 1, 255);
const input = bytes(3, 4, 5);
await kv.set(key, input);
input[0] = 99;

const first = await kv.get(key);
assertEquals(Array.from(first), [3, 4, 5]);
first[1] = 99;
assertEquals(Array.from(await kv.get(key)), [3, 4, 5]);

await kv.set(key, bytes());
assertEquals(Array.from(await kv.get(key)), []);
await kv.delete(key);
assertEquals(await kv.get(key), null);

await assertRejects(() => kv.get(bytes()));
await assertRejects(() => kv.get(new Uint8Array(2049)));
await assertRejects(() => kv.set(bytes(1), new Uint8Array(65537)));

const donor = new ArrayBuffer(1);
const detached = new Uint8Array(donor);
donor.transfer();
await assertRejects(() => kv.get(detached));

await Promise.all([kv.close(), kv.close(), kv.close()]);
await assertRejects(() => kv.get(key));

const reopened = await Kv.open(path);
assertEquals(await reopened.get(key), null);
await reopened.close();
