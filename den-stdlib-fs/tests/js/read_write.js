import { assertEquals, assertRejects } from "den:assert";
import {
  canonicalize,
  copy,
  metadata,
  read,
  readToString,
  removeFile,
  write,
} from "den:fs";

const dir = process.env.DEN_TEST_DIR;
const file = process.env.DEN_TEST_FILE;
const out = `${dir}/out.txt`;
const copied = `${dir}/copy.txt`;

assertEquals(await readToString(file), "hello");
const bytes = await read(file);
assertEquals(Array.from(bytes), [104, 101, 108, 108, 111]);

await write(out, Array.from(new TextEncoder().encode("world")));
assertEquals(await readToString(out), "world");
await copy(out, copied);
assertEquals(await readToString(copied), "world");

const canonical = await canonicalize(file);
assertEquals(typeof canonical, "string");
assertEquals(canonical.endsWith("hello.txt"), true);

await removeFile(out);
await assertRejects(() => metadata(out));
await removeFile(copied);
