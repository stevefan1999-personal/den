// cargo run -- examples/spawn.ts
//
// process.spawn is tokio::process::Command with piped stdio readers.
// lookup is tokio::net::lookup_host.

import { metadata } from "den:fs";
import { env, lookup, spawn } from "den:process";

async function firstExisting(paths: string[]): Promise<string> {
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

const command = env.ComSpec
  ? [env.ComSpec, "/d", "/s", "/c", "echo hello-from-den"]
  : [await firstExisting(["/bin/echo", "/usr/bin/echo"]), "hello-from-den"];
const child = spawn(command, { stdout: "pipe", stderr: "ignore" });
const out = await child.stdout?.text();
const status = await child.wait();
console.log("pid", child.pid, "code", status.code, "stdout", out?.trim());

const addr = await lookup("localhost");
if (!Array.isArray(addr)) {
  console.log("lookup localhost", addr.ip, "family", addr.family);
}
