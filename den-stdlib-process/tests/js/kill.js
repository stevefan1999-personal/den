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

const sleep = await firstExisting(["/bin/sleep", "/usr/bin/sleep"]);
const child = process.spawn([sleep, "30"], { stdout: "ignore", stderr: "ignore" });
process.kill(child.pid, "SIGKILL");
const status = await child.wait();
assert(child.pid > 0);
assertEquals(status.code, null);
