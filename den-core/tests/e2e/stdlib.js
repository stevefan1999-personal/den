import { assert, assertEquals } from "den:assert";
import { metadata, write, createDirAll } from "den:fs";
import { Connection } from "den:sqlite";
import * as net from "den:networking";

assertEquals(typeof console.log, "function");
assertEquals(atob(btoa("den")), "den");
assertEquals(typeof crypto.randomUUID(), "string");
assertEquals(new TextDecoder().decode(new TextEncoder().encode("ok")), "ok");
assert(Temporal.Now.instant() instanceof Temporal.Instant);
assertEquals(typeof setTimeout, "function");
assertEquals(typeof process.pid, "number");
assert(new Blob(["x"]) instanceof Blob);
assertEquals(new Headers({ a: "b" }).get("a"), "b");
assertEquals(typeof fetch, "function");
assertEquals(typeof Worker, "function");
if (typeof WebAssembly === "object" && WebAssembly) {
  assertEquals(typeof WebAssembly.validate, "function");
}
assertEquals(typeof net.TcpListener, "function");

const dir = `${process.env.TMPDIR ?? process.env.TEMP ?? "/tmp"}/den-e2e-${process.pid}`;
await createDirAll(dir);
assertEquals(typeof metadata, "function");
assertEquals(typeof write, "function");

const open = Connection.openInMemory ?? Connection.open_in_memory;
const db = open.call(Connection);
(db.execute.bind(db))("CREATE TABLE t (n INTEGER)");
(db.execute.bind(db))("INSERT INTO t VALUES (1)");
db.close();

await new Promise((resolve) => setTimeout(resolve, 1));
