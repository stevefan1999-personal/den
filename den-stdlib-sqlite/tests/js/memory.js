import { assertEquals, assert } from "den:assert";
import { Connection } from "den:sqlite";

const open = Connection.openInMemory ?? Connection.open_in_memory;
const db = open.call(Connection);
const execute = db.execute.bind(db);
const query = (db.queryRows ?? db.query_rows).bind(db);
execute("CREATE TABLE t (id INTEGER, name TEXT)");
execute("INSERT INTO t VALUES (?, ?)", [1, "ada"]);
execute("INSERT INTO t VALUES (:id, :name)", { id: 2, name: "grace" });
const rows = query("SELECT id, name FROM t ORDER BY id");
assert(Array.isArray(rows));
assertEquals(rows.length, 2);
assertEquals(Number(rows[0].id ?? rows[0][0]), 1);
db.close();
