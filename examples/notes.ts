import { Connection } from "den:sqlite";
import { renderToStaticMarkup } from "https://esm.sh/react-dom@18.3.1/server";
import { route, type HttpReply, type HttpRequest } from "./http.ts";
import { Page, type Note } from "./site.tsx";

export interface Sqlite {
  close(): void;
  execute(sql: string, params?: unknown[]): number;
  queryRows?(sql: string, params?: unknown[]): unknown[][] | null;
  query_rows?(sql: string, params?: unknown[]): unknown[][] | null;
}

export interface WireRequest {
  method: string;
  path: string;
  body: string;
}

export function openNotes(path: string): Sqlite {
  return Connection.open(path) as unknown as Sqlite;
}

export function query(db: Sqlite, sql: string, params?: unknown[]): unknown[][] {
  const rowsFn = db.queryRows ?? db.query_rows;
  if (rowsFn === undefined) {
    throw new TypeError("connection has no queryRows");
  }
  const rows = params === undefined ? rowsFn.call(db, sql) : rowsFn.call(db, sql, params);
  return rows ?? [];
}

export function now(): string {
  return new Date().toISOString();
}

export function ensureSchema(db: Sqlite): void {
  query(db, "PRAGMA journal_mode=WAL");
  db.execute(
    "CREATE TABLE IF NOT EXISTS notes (id INTEGER PRIMARY KEY AUTOINCREMENT, body TEXT NOT NULL, created TEXT NOT NULL)",
  );
  if (query(db, "SELECT COUNT(*) FROM notes")[0]?.[0] === 0) {
    db.execute("INSERT INTO notes (body, created) VALUES (?, ?)", [
      "Hello from den. This row lives in SQLite.",
      now(),
    ]);
  }
}

function notesFrom(db: Sqlite): Note[] {
  return query(db, "SELECT id, body, created FROM notes ORDER BY id DESC").map((row) => ({
    id: Number(row[0]),
    body: String(row[1]),
    created: String(row[2]),
  }));
}

function html(node: unknown): HttpReply {
  return {
    status: 200,
    reason: "OK",
    type: "text/html; charset=utf-8",
    body: `<!DOCTYPE html>${renderToStaticMarkup(node as Parameters<typeof renderToStaticMarkup>[0])}`,
  };
}

function json(value: unknown): HttpReply {
  return {
    status: 200,
    reason: "OK",
    type: "application/json; charset=utf-8",
    body: JSON.stringify(value),
  };
}

export function handle(db: Sqlite, request: HttpRequest | WireRequest): HttpReply {
  if (request.method === "OPTIONS") {
    return { status: 204, reason: "No Content", type: "text/plain", body: "" };
  }
  if (request.method === "GET" && route(request.path, "/favicon.ico")) {
    return { status: 204, reason: "No Content", type: "image/x-icon", body: "" };
  }
  if (request.method === "GET" && route(request.path, "/")) {
    return html(Page({ notes: notesFrom(db) }));
  }
  if (request.method === "GET" && route(request.path, "/json")) {
    return json({ notes: notesFrom(db) });
  }
  if (request.method === "POST" && route(request.path, "/notes")) {
    const body = new URLSearchParams(request.body).get("body")?.trim() ?? "";
    if (body.length > 0) {
      db.execute("INSERT INTO notes (body, created) VALUES (?, ?)", [body, now()]);
    }
    return {
      status: 303,
      reason: "See Other",
      type: "text/plain",
      body: "",
      extra: ["Location: /"],
    };
  }
  return { status: 404, reason: "Not Found", type: "text/plain; charset=utf-8", body: "not found" };
}
