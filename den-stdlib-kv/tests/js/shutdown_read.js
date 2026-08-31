import { assertEquals } from "den:assert";
import { Kv } from "den:kv";

const kv = await Kv.open(__STORE__);
assertEquals(Array.from(await kv.get(new Uint8Array([1]))), [7, 8, 9]);
await kv.close();
