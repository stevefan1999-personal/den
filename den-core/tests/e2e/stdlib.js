import { assert, assertEquals } from "den:assert";
import { metadata, write, createDirAll } from "den:fs";
import { serve } from "den:http";
import { Kv } from "den:kv";
import * as net from "den:networking";
import path from "den:path";

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
assertEquals(typeof Kv.open, "function");
assertEquals(typeof serve, "function");
assertEquals(path.posix.normalize("/srv/app/../data"), "/srv/data");
assertEquals(path.windows.join("C:\\srv", "data"), "C:\\srv\\data");

const dir = `${process.env.TMPDIR ?? process.env.TEMP ?? "/tmp"}/den-e2e-${process.pid}`;
await createDirAll(dir);
assertEquals(typeof metadata, "function");
assertEquals(typeof write, "function");

await new Promise((resolve) => setTimeout(resolve, 1));
