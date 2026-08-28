import { handle, openNotes, type Sqlite } from "./notes.ts";

let db: Sqlite | undefined;

self.onmessage = (event: MessageEvent) => {
  const data = event.data;
  if (data?.type === "init") {
    db = openNotes(String(data.dbPath));
    postMessage({ type: "ready" });
    return;
  }
  if (db === undefined) {
    postMessage({ id: data?.id, error: "worker has no database" });
    return;
  }
  try {
    postMessage({
      id: data.id,
      reply: handle(db, {
        method: String(data.method ?? "GET"),
        path: String(data.path ?? "/"),
        body: String(data.body ?? ""),
      }),
    });
  } catch (error) {
    postMessage({ id: data.id, error: String(error) });
  }
};
