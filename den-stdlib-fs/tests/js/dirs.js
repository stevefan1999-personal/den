import { assert, assertEquals, assertRejects } from "den:assert";
import {
  createDir,
  createDirAll,
  metadata,
  readDir,
  removeDir,
  removeDirAll,
  rename,
} from "den:fs";

const dir = process.env.DEN_TEST_DIR;
const nested = `${dir}/a/b`;
const leaf = `${dir}/a/b/c`;
const renamed = `${dir}/renamed`;

await createDirAll(leaf);
assertEquals((await metadata(leaf)).isDir, true);
await createDir(`${dir}/empty`);
const names = (await readDir(dir)).map((entry) => entry.name);
assert(names.includes("empty"));
assert(names.includes("a"));

await rename(`${dir}/empty`, renamed);
assertEquals((await metadata(renamed)).isDir, true);
await removeDir(renamed);
await assertRejects(() => metadata(renamed));

await removeDirAll(`${dir}/a`);
await assertRejects(() => metadata(nested));
