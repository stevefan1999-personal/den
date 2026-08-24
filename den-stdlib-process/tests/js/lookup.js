import { assert } from "den:assert";
const addr = await process.lookup("localhost");
assert(addr.ip === "127.0.0.1" || addr.ip === "::1");
const addrs = await process.lookup("localhost", { all: true });
assert(Array.isArray(addrs) && addrs.length > 0);
assert(addrs.every((item) => item.ip && (item.family === 4 || item.family === 6)));
