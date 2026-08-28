import { assert, assertEquals } from "den:assert";
import {
  createDirAll,
  metadata,
  read,
  readToString,
  removeDirAll,
  write,
} from "den:fs";
import { posix } from "den:path";
import { tempDir } from "./lib/temp.js";

const dir = tempDir("notes");
await createDirAll(dir);
const file = posix.join(dir, "note.txt");
await write(file, Array.from(new TextEncoder().encode("hello den")));

assertEquals(await readToString(file), "hello den");
const stat = await metadata(file);
assert(stat.isFile);
assertEquals(Number(stat.len), 9);
assertEquals(new TextDecoder().decode(new Uint8Array(await read(file))), "hello den");

await removeDirAll(dir);
