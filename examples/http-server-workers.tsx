// cargo run -- examples/http-server-workers.tsx
//
// Same site as http-server.tsx, but each request's SQLite + React work runs
// on a pool of module workers (one OS thread / Engine each). Sockets stay
// on the parent: TcpStream is not transferable.

import { addSignalListener, cwd, env, exit } from "den:process";
import { TcpListener } from "den:networking";
import { encode, readRequest, type HttpReply } from "./http.ts";
import { connections, dest, writeAll, type ByteStream } from "./net.ts";
import { ensureSchema, openNotes } from "./notes.ts";

addSignalListener("SIGINT", () => exit(0));
addSignalListener("SIGTERM", () => exit(0));

interface WorkerReply {
  type?: string;
  id?: number;
  reply?: HttpReply;
  error?: string;
}

function poolSize(): number {
  const fromEnv = Number(env.WORKERS ?? "");
  if (Number.isFinite(fromEnv) && fromEnv >= 1) {
    return Math.min(Math.floor(fromEnv), 32);
  }
  const cores = Number(globalThis.navigator?.hardwareConcurrency ?? 4);
  return Math.max(1, Math.min(cores, 32));
}

function bindWorker(worker: Worker): {
  ready: Promise<void>;
  ask: (method: string, path: string, body: string) => Promise<HttpReply>;
} {
  const pending = new Map<
    number,
    { resolve: (reply: HttpReply) => void; reject: (error: Error) => void }
  >();
  let nextId = 1;
  let settleReady: () => void;
  const ready = new Promise<void>((resolve) => {
    settleReady = resolve;
  });

  worker.onmessage = (event: MessageEvent<WorkerReply>) => {
    const data = event.data;
    if (data?.type === "ready") {
      settleReady();
      return;
    }
    const id = data?.id;
    if (id === undefined) {
      return;
    }
    const slot = pending.get(id);
    if (slot === undefined) {
      return;
    }
    pending.delete(id);
    if (data.error !== undefined) {
      slot.reject(new Error(data.error));
      return;
    }
    if (data.reply === undefined) {
      slot.reject(new Error("worker returned no reply"));
      return;
    }
    slot.resolve(data.reply);
  };
  worker.onerror = (event: ErrorEvent) => {
    const error = new Error(event.message);
    for (const slot of pending.values()) {
      slot.reject(error);
    }
    pending.clear();
  };

  return {
    ready,
    ask(method: string, path: string, body: string): Promise<HttpReply> {
      const id = nextId;
      nextId += 1;
      return new Promise((resolve, reject) => {
        pending.set(id, { resolve, reject });
        worker.postMessage({ id, method, path, body });
      });
    },
  };
}

async function bootPool(dbPath: string): Promise<
  Array<{ ask: (method: string, path: string, body: string) => Promise<HttpReply> }>
> {
  const size = poolSize();
  const workers = Array.from({ length: size }, (_, index) => {
    const worker = new Worker("./http-worker.ts", {
      type: "module",
      name: `http-${index}`,
    });
    const bound = bindWorker(worker);
    worker.postMessage({ type: "init", dbPath });
    return bound;
  });
  await Promise.all(workers.map((worker) => worker.ready));
  console.log("workers", size);
  return workers;
}

const port = env.PORT ?? "8080";
const dbPath = `${cwd()}/notes.db`;
const db = openNotes(dbPath);
ensureSchema(db);

const pool = await bootPool(dbPath);
const listener = await TcpListener.listen(`0.0.0.0:${port}`);
console.log(`Open http://${dest(listener)}  (Ctrl+C to stop)`);
console.log("sqlite", dbPath);

let next = 0;
for await (const { stream, peer } of connections(listener)) {
  const worker = pool[next % pool.length];
  next += 1;
  serve(worker, stream, peer).catch((error: unknown) => {
    console.error(error);
  });
}

async function serve(
  worker: { ask: (method: string, path: string, body: string) => Promise<HttpReply> },
  stream: ByteStream,
  peer: { toString(): string },
): Promise<void> {
  try {
    const request = await readRequest(stream);
    console.log(request.method, request.path, "from", peer.toString());
    const reply = await worker.ask(request.method, request.path, request.body);
    await writeAll(stream, encode(reply));
  } catch (error) {
    console.error(error);
  } finally {
    await stream.shutdown();
  }
}
