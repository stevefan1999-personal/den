import { assert, assertEquals, assertThrows } from "den:assert";
import { Connection } from "den:sqlite";

const open = Connection.openInMemory ?? Connection.open_in_memory;
const db = open.call(Connection);
const execute = db.execute.bind(db);
const query = (db.queryRows ?? db.query_rows).bind(db);

execute("CREATE TABLE t (i INTEGER, r REAL, s TEXT, n INTEGER)");
execute("INSERT INTO t VALUES (?, ?, ?, ?)", [1, 1.5, "hi", null]);
execute("INSERT INTO t VALUES (2147483648, 0, 'big', NULL)");

const rows = query("SELECT i, r, s, n FROM t ORDER BY rowid");
assertEquals(rows.length, 2);
assertEquals(rows[0][0], 1);
assert(rows[0][1] > 1 && rows[0][1] < 2);
assertEquals(rows[0][2], "hi");
assertEquals(rows[0][3], null);
assertEquals(typeof rows[1][0], "bigint");
assertEquals(rows[1][0], 2147483648n);

const empty = query("SELECT i FROM t WHERE 0");
assert(empty == null);

assertThrows(() => query("SELECT ?", [1, 2]), Error, "too many parameters");
db.close();
