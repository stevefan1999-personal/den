/// <reference path="../types/den-kv.d.ts" />

// cargo run -- examples/kv.ts
//
// den:kv is SurrealKV: byte keys, byte values, immediate durability on
// resolved mutations, and snapshot transactions. commit() returns false when
// the snapshot conflicted — begin a new transaction instead of retrying the
// same handle.

import { assertEquals } from "den:assert";
import { Kv } from "den:kv";
import { uniquePath } from "./temp.ts";

const bytes = (...values: number[]): Uint8Array => new Uint8Array(values);
const path = uniquePath("den-example-kv");
const kv = await Kv.open(path);
const key = bytes(1, 2, 3);

await kv.set(key, bytes(9, 8, 7));
const first = Array.from((await kv.get(key)) ?? []);
assertEquals(first, [9, 8, 7]);
console.log("get", first);

const transaction = await kv.transaction();
await transaction.set(key, bytes(4, 5, 6));
const committed = await transaction.commit();
const afterCommit = Array.from((await kv.get(key)) ?? []);
assertEquals(committed, true);
assertEquals(afterCommit, [4, 5, 6]);
console.log("committed", committed, "now", afterCommit);

const stale = await kv.transaction();
const writer = await kv.transaction();
await writer.set(key, bytes(1));
await writer.commit();
await stale.set(key, bytes(2));
const staleCommit = await stale.commit();
assertEquals(staleCommit, false);
console.log("stale commit", staleCommit);

await kv.close();
console.log("store", path);
