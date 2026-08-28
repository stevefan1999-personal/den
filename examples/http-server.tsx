// cargo run -- examples/http-server.tsx
//
// Stays up until Ctrl+C. Open http://127.0.0.1:8080
// One QuickJS realm does accept + SQLite + React. For a worker pool, see
// http-server-workers.tsx.

import { addSignalListener, cwd, env, exit } from "den:process";
import { TcpListener } from "den:networking";
import { encode, readRequest } from "./http.ts";
import { connections, dest, writeAll, type ByteStream } from "./net.ts";
import { ensureSchema, handle, openNotes, type Sqlite } from "./notes.ts";

addSignalListener("SIGINT", () => exit(0));
addSignalListener("SIGTERM", () => exit(0));

async function serve(db: Sqlite, stream: ByteStream, peer: { toString(): string }): Promise<void> {
  try {
    const request = await readRequest(stream);
    console.log(request.method, request.path, "from", peer.toString());
    await writeAll(stream, encode(handle(db, request)));
  } catch (error) {
    console.error(error);
  } finally {
    await stream.shutdown();
  }
}

const port = env.PORT ?? "8080";
const dbPath = `${cwd()}/notes.db`;
const db = openNotes(dbPath);
ensureSchema(db);

const listener = await TcpListener.listen(`0.0.0.0:${port}`);
console.log(`Open http://${dest(listener)}  (Ctrl+C to stop)`);
console.log("sqlite", dbPath);

for await (const { stream, peer } of connections(listener)) {
  serve(db, stream, peer).catch((error: unknown) => {
    console.error(error);
  });
}
