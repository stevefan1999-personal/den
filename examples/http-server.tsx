/// <reference path="../types/den-http.d.ts" />

// cargo run -- examples/http-server.tsx
// Open http://127.0.0.1:8080 and press Ctrl+C to stop.

import { serve } from "den:http";
import { addSignalListener, cwd, env, exit } from "den:process";
import { fromRequest, toResponse } from "./http.ts";
import { ensureSchema, handle, openNotes } from "./notes.ts";

const dbPath = `${cwd()}/notes.db`;
const db = openNotes(dbPath);
ensureSchema(db);

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
    return toResponse(handle(db, input));
  },
});

async function shutdown(): Promise<void> {
  await server.close();
  exit(0);
}

addSignalListener("SIGINT", () => void shutdown());
addSignalListener("SIGTERM", () => void shutdown());
console.log(`Open ${server.url}  (Ctrl+C to stop)`);
console.log("sqlite", dbPath);
await server.finished;
