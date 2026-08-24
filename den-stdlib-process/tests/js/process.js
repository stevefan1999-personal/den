import { assert, assertEquals } from "den:assert";
assertEquals(typeof process.pid, "number");
assert(process.pid > 0);
assert(typeof process.ppid === "number" && process.ppid > 0);
assert(Array.isArray(process.argv) && process.argv.length > 0);
assert(process.argv.every((arg) => typeof arg === "string"));
assertEquals(typeof (process.env.PATH ?? process.env.HOME), "string");
assertEquals(typeof process.cwd, "function");
assertEquals(typeof process.exit, "function");
