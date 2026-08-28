import { assert, assertEquals } from "den:assert";
import { Connection } from "den:sqlite";
import { renderToStaticMarkup } from "https://esm.sh/react-dom@18.3.1/server";
import { Page, type Note } from "../../../examples/site.tsx";

const open = Connection.openInMemory ?? Connection.open_in_memory;
const db = open.call(Connection);
const execute = db.execute.bind(db);
const query = (db.queryRows ?? db.query_rows).bind(db);

execute(
  "CREATE TABLE notes (id INTEGER PRIMARY KEY AUTOINCREMENT, body TEXT NOT NULL, created TEXT NOT NULL)",
);
execute("INSERT INTO notes (body, created) VALUES (?, ?)", [
  "<script>alert(1)</script>",
  "2026-08-27T00:00:00.000Z",
]);

const rows = query("SELECT id, body, created FROM notes");
assertEquals(rows.length, 1);
const notes: Note[] = [
  { id: Number(rows[0][0]), body: String(rows[0][1]), created: String(rows[0][2]) },
];

const html = `<!DOCTYPE html>${renderToStaticMarkup(Page({ notes }))}`;
assert(html.startsWith("<!DOCTYPE html>"));
assert(html.includes("Notes"));
assert(html.includes("&lt;script&gt;alert(1)&lt;/script&gt;"));
assert(!html.includes("<script>alert(1)</script>"));
db.close();
