import { assert, assertEquals, assertThrows } from "den:assert";
import { Connection } from "den:sqlite";

const open = Connection.openInMemory ?? Connection.open_in_memory;
const db = open.call(Connection);
const execute = db.execute.bind(db);
const query = (db.queryRows ?? db.query_rows).bind(db);

execute("CREATE TABLE t (i INTEGER, r REAL, s TEXT, n INTEGER, b BLOB)");
execute("INSERT INTO t VALUES (?, ?, ?, ?, ?)", [1, 1.5, "hi", null, new Uint8Array([1, 2, 255])]);
execute("INSERT INTO t VALUES (?, 0, 'big', NULL, ?)", [2147483648n, new ArrayBuffer(0)]);

const rows = query("SELECT i, r, s, n, b FROM t ORDER BY rowid");
assertEquals(rows.length, 2);
assertEquals(rows[0][0], 1);
assert(rows[0][1] > 1 && rows[0][1] < 2);
assertEquals(rows[0][2], "hi");
assertEquals(rows[0][3], null);
assert(rows[0][4] instanceof Uint8Array);
assertEquals([...rows[0][4]], [1, 2, 255]);
assertEquals(typeof rows[1][0], "bigint");
assertEquals(rows[1][0], 2147483648n);

const empty = query("SELECT i FROM t WHERE 0");
assert(empty == null);

assertThrows(() => query("SELECT ?", [1, 2]), Error, "too many parameters");
db.close();
