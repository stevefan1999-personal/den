import { assertEquals } from "den:assert";
import { createDirAll, metadata, readToString, removeDirAll, write } from "den:fs";
import { posix } from "den:path";
import { tempDir } from "./lib/temp.js";

async function firstExisting(paths) {
  for (const path of paths) {
    try {
      await metadata(path);
      return path;
    } catch {
      /* keep looking */
    }
  }
  return paths[0];
}

const command = process.env.ComSpec
  ? [process.env.ComSpec, "/d", "/s", "/c", "echo hello-from-child"]
  : [await firstExisting(["/bin/echo", "/usr/bin/echo"]), "hello-from-child"];
const child = process.spawn(command, {
  stdout: "pipe",
  stderr: "ignore",
});
const out = await child.stdout.text();
const status = await child.wait();
assertEquals(status.code, 0);

const dir = tempDir("spawn");
await createDirAll(dir);
const file = posix.join(dir, "out.txt");
await write(file, Array.from(new TextEncoder().encode(out.trim())));
assertEquals(await readToString(file), "hello-from-child");
await removeDirAll(dir);
