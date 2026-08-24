import { assertEquals } from "den:assert";
import {
  metadata,
  readDir,
  readLink,
  symlinkMetadata,
  setPermissions,
} from "den:fs";

const dir = process.env.DEN_TEST_DIR;
const file = process.env.DEN_TEST_FILE;
const sub = process.env.DEN_TEST_SUB;
const link = process.env.DEN_TEST_LINK;

const meta = await metadata(file);
assertEquals(meta.len, 5);
assertEquals(meta.isFile, true);
assertEquals(meta.isDir, false);
assertEquals(meta.isSymlink, false);

const dirMeta = await metadata(sub);
assertEquals(dirMeta.isDir, true);
assertEquals(dirMeta.isFile, false);

const entries = await readDir(dir);
const byName = Object.fromEntries(entries.map((entry) => [entry.name, entry]));
assertEquals(byName["hello.txt"]?.isFile, true);
assertEquals(byName["sub"]?.isDir, true);

if (process.env.DEN_TEST_UNIX === "1") {
  const linked = await readLink(link);
  assertEquals(linked, "hello.txt");
  const linkMeta = await symlinkMetadata(link);
  assertEquals(linkMeta.isSymlink, true);
  const followed = await metadata(link);
  assertEquals(followed.isFile, true);
  assertEquals(followed.isSymlink, false);
  await setPermissions(file, 0o600);
  const chmodded = await metadata(file);
  assertEquals(chmodded.mode & 0o777, 0o600);
  assertEquals(byName["hello.link"]?.isSymlink, true);
}
