// cargo run -- examples/fs-path.ts
//
// den:path is lexical (no I/O). den:fs is the host filesystem. write() takes
// an array of bytes; pass `{ atomic: true }` to rename a sibling temp file
// onto the target so a crash cannot leave a truncated prefix.

import { assert, assertEquals } from "den:assert";
import {
  createDirAll,
  metadata,
  readToString,
  removeDirAll,
  write,
} from "den:fs";
import { posix } from "den:path";
import { uniquePath, utf8Bytes } from "./temp.ts";

const dir = uniquePath("den-example-fs");
const file = posix.join(dir, "note.txt");
await createDirAll(dir);
await write(file, utf8Bytes("hello den"), { atomic: true });

const stat = await metadata(file);
const text = await readToString(file);
assertEquals(posix.basename(file), "note.txt");
assert(stat.isFile);
assertEquals(Number(stat.len), 9);
assertEquals(text, "hello den");
console.log("path", posix.basename(file), "in", posix.dirname(file));
console.log("stat", { isFile: stat.isFile, len: Number(stat.len) });
console.log("text", text);

await removeDirAll(dir);
