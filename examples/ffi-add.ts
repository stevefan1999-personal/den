/// <reference path="../types/den-ffi.d.ts" />

// cargo run -- --allow-ffi examples/ffi-add.ts
//
// den:ffi is denied at runtime without a grant, even when the crate is
// compiled in. --allow-ffi (optionally =PATH) mints the capability. The
// schema is data: it types the call site and builds the libffi CIF.

import { assertEquals } from "den:assert";
import { grant, open, suffix } from "den:ffi";
import { metadata } from "den:fs";
import { posix } from "den:path";
import { cwd, env, spawn } from "den:process";
import { uniquePath } from "./temp.ts";

const capability = grant();
if (capability === null) {
  console.log("den:ffi is capability-gated. Re-run with:");
  console.log("  cargo run -- --allow-ffi examples/ffi-add.ts");
} else {
  const source = posix.join(cwd(), "examples/ffi-add.c");
  await metadata(source);
  const library = uniquePath("libadd") + suffix;
  const compiler = env.CC ?? env.DEN_TEST_CC ?? "cc";
  const child = spawn([compiler, "-shared", "-fPIC", "-o", library, source], {
    stdout: "pipe",
    stderr: "pipe",
  });
  const [err, status] = await Promise.all([
    child.stderr?.text() ?? Promise.resolve(""),
    child.wait(),
  ]);
  if (status.code !== 0) {
    throw new Error(`${compiler} failed (${status.code}): ${err}`);
  }

  const probe = open(library, {
    add: { params: ["i32", "i32"], result: "i32" },
  }, capability);
  assertEquals(probe.add(40, 2), 42);
  console.log("ffi add", probe.add(40, 2));
  probe[Symbol.dispose]();
}
