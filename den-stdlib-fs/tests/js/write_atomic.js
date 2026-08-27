import { assertEquals } from "den:assert";
import { createDir, readDir, readToString, write } from "den:fs";

const dir = `${process.env.DEN_TEST_DIR}/atomic`;
const target = `${dir}/target.txt`;
const bytes = (text) => Array.from(new TextEncoder().encode(text));

await createDir(dir);

// Default path: unchanged truncate-then-write.
await write(target, bytes("old"));
assertEquals(await readToString(target), "old");
await write(target, bytes("older"), { atomic: false });
assertEquals(await readToString(target), "older");

await write(target, bytes("new"), { atomic: true });
assertEquals(await readToString(target), "new");

// The rename consumed the temporary file, so the directory holds the target alone.
assertEquals((await readDir(dir)).map((entry) => entry.name), ["target.txt"]);
