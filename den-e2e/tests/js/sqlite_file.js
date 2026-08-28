import { assert, assertEquals } from "den:assert";
import { createDirAll, metadata, removeDirAll } from "den:fs";
import { posix } from "den:path";
import { Connection } from "den:sqlite";
import { tempDir } from "./lib/temp.js";

const dir = tempDir("sqlite");
await createDirAll(dir);
const dbPath = posix.join(dir, "notes.db");

const db = Connection.open(dbPath);
const execute = db.execute.bind(db);
const query = (db.queryRows ?? db.query_rows).bind(db);

execute("CREATE TABLE notes (id INTEGER PRIMARY KEY, body TEXT)");
execute("INSERT INTO notes (body) VALUES (?)", ["hello den"]);
const rows = query("SELECT body FROM notes");
assertEquals(rows[0].body ?? rows[0][0], "hello den");
db.close();

assert((await metadata(dbPath)).isFile);
await removeDirAll(dir);
