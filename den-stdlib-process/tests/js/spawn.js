import { assertEquals, assert } from "den:assert";
import { metadata } from "den:fs";

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

const echo = await firstExisting(["/bin/echo", "/usr/bin/echo"]);
const child = process.spawn([echo, "hello-from-den"], { stdout: "pipe", stderr: "ignore" });
const out = await child.stdout.text();
const status = await child.wait();
assert(child.pid > 0);
assertEquals(status.code, 0);
assertEquals(out.trim(), "hello-from-den");
