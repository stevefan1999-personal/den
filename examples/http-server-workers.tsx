/// <reference path="../types/den-http.d.ts" />

// cargo run -- examples/http-server-workers.tsx
// The HTTP server stays in the main realm; SQLite and React run in workers.

import { serve } from "den:http";
import { addSignalListener, cwd, env, exit } from "den:process";
import { fromRequest, type HttpReply, toResponse } from "./http.ts";
import { ensureSchema, openNotes } from "./notes.ts";

interface WorkerReply {
  type?: string;
  id?: number;
  reply?: HttpReply;
  error?: string;
}

function bindWorker(worker: Worker): {
  ready: Promise<void>;
  ask(method: string, path: string, body: string): Promise<HttpReply>;
} {
  const pending = new Map<
    number,
    { resolve: (reply: HttpReply) => void; reject: (error: Error) => void }
  >();
  let nextId = 1;
  let ready!: () => void;
  const started = new Promise<void>((resolve) => (ready = resolve));

  worker.onmessage = ({ data }: MessageEvent<WorkerReply>) => {
    if (data?.type === "ready") return ready();
    const slot = data?.id === undefined ? undefined : pending.get(data.id);
    if (slot === undefined) return;
    pending.delete(data.id!);
    if (data.error !== undefined) slot.reject(new Error(data.error));
    else if (data.reply !== undefined) slot.resolve(data.reply);
    else slot.reject(new Error("worker returned no reply"));
  };
  worker.onerror = ({ message }: ErrorEvent) => {
    for (const slot of pending.values()) slot.reject(new Error(message));
    pending.clear();
  };

  return {
    ready: started,
    ask(method, path, body) {
      const id = nextId++;
      return new Promise((resolve, reject) => {
        pending.set(id, { resolve, reject });
        worker.postMessage({ id, method, path, body });
      });
    },
  };
}

const dbPath = `${cwd()}/notes.db`;
const db = openNotes(dbPath);
ensureSchema(db);

const requested = Number(env.WORKERS ?? navigator.hardwareConcurrency ?? 4);
const count = Number.isFinite(requested)
  ? Math.max(1, Math.min(Math.floor(requested), 32))
  : 4;
const pool = Array.from({ length: count }, (_, index) => {
  const worker = new Worker("./http-worker.ts", {
    type: "module",
    name: `http-${index}`,
  });
  const bound = bindWorker(worker);
  worker.postMessage({ type: "init", dbPath });
  return bound;
});
await Promise.all(pool.map((worker) => worker.ready));

let next = 0;
const server = serve({
  listen: { host: "0.0.0.0", port: Number(env.PORT ?? "8080") },
  async fetch(request, connection) {
    const input = await fromRequest(request);
    console.log(
      input.method,
      input.path,
      "from",
      `${connection.remote.hostname}:${connection.remote.port}`,
    );
    const worker = pool[next++ % pool.length];
    return toResponse(await worker.ask(input.method, input.path, input.body));
  },
});

async function shutdown(): Promise<void> {
  await server.close();
  exit(0);
}

addSignalListener("SIGINT", () => void shutdown());
addSignalListener("SIGTERM", () => void shutdown());
console.log(`Open ${server.url} with ${count} workers  (Ctrl+C to stop)`);
console.log("sqlite", dbPath);
await server.finished;
