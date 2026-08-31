import { assert, assertEquals, assertRejects } from "den:assert";
import { Kv } from "den:kv";

const kv = await Kv.open(__STORE__);
const tx = await kv.transaction();
const value = new Uint8Array(65536);
let rejected = false;

for (let index = 0; index < 300; index++) {
  const key = new Uint8Array([1, index >> 8, index & 255]);
  try {
    await tx.set(key, value);
  } catch (error) {
    rejected = error instanceof RangeError;
    break;
  }
}

assert(rejected, "aggregate transaction limit must reject before commit");
assert(await tx.commit());
assertEquals(Array.from((await kv.get(new Uint8Array([1, 0, 0]))).slice(0, 1)), [0]);
await kv.close();

const reopened = await Kv.open(__STORE__);
assertEquals(Array.from((await reopened.get(new Uint8Array([1, 0, 0]))).slice(0, 1)), [0]);

const entries = await reopened.transaction();
let entriesRejected = false;
for (let index = 0; index <= 1000; index++) {
  const key = new Uint8Array([2, index >> 8, index & 255]);
  try {
    await entries.delete(key);
  } catch (error) {
    entriesRejected = error instanceof RangeError;
    break;
  }
}
assert(entriesRejected, "aggregate transaction entry limit must reject before commit");
await entries.rollback();
await reopened.close();
