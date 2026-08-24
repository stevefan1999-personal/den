const { assert, assertEquals } = await import("den:assert");
assertEquals(typeof process.pid, "number");
assert(process.pid > 0);
assert(Array.isArray(process.argv) && process.argv.length > 0);
assertEquals(typeof (process.env.PATH ?? process.env.HOME), "string");
assertEquals(typeof process.cwd, "function");
