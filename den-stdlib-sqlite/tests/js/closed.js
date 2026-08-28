import { assertEquals, assertThrows } from "den:assert";
import { Connection } from "den:sqlite";

const open = Connection.openInMemory ?? Connection.open_in_memory;
const db = open.call(Connection);
const execute = db.execute.bind(db);
assertEquals(execute("CREATE TABLE t (id INTEGER)"), 0);
assertEquals(execute("INSERT INTO t VALUES (1)"), 1);
db.close();
assertThrows(() => execute("SELECT 1"), Error, "already closed");
assertThrows(() => db.close(), Error, "already closed");
