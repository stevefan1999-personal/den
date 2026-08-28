import { assert, assertEquals } from "den:assert";
import { createDirAll, removeDirAll } from "den:fs";
import { posix } from "den:path";
import { Connection } from "den:sqlite";
import { tempDir } from "./lib/temp.js";
import { ensureSchema, openNotes } from "../../../examples/notes.ts";

const dir = tempDir("http-worker");
await createDirAll(dir);
const dbPath = posix.join(dir, "notes.db");
ensureSchema(openNotes(dbPath));

const worker = new Worker("../../../examples/http-worker.ts", { type: "module" });
const reply = await new Promise<Record<string, unknown>>((resolve, reject) => {
  worker.onmessage = (event: MessageEvent) => resolve(event.data);
  worker.onerror = (event: ErrorEvent) => reject(new Error(event.message));
  worker.postMessage({ type: "init", dbPath });
});
assertEquals(reply.type, "ready");

const handled = await new Promise<Record<string, unknown>>((resolve, reject) => {
  worker.onmessage = (event: MessageEvent) => resolve(event.data);
  worker.onerror = (event: ErrorEvent) => reject(new Error(event.message));
  worker.postMessage({ id: 1, method: "GET", path: "/", body: "" });
});
worker.terminate();

assertEquals(handled.id, 1);
const body = (handled.reply as { body: string }).body;
assert(body.startsWith("<!DOCTYPE html>"));
assert(body.includes("Notes"));
assert(body.includes("Hello from den"));

const check = Connection.open(dbPath);
check.close();
await removeDirAll(dir);
