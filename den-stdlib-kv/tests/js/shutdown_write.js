import { Kv } from "den:kv";

const kv = await Kv.open(__STORE__);
await kv.set(new Uint8Array([1]), new Uint8Array([7, 8, 9]));
// Engine::shutdown owns the close; deliberately leave `kv` open.
