import { assertEquals } from "den:assert";
import { createDirAll, canonicalize } from "den:fs";

const original = process.cwd();
const dir = `${process.env.TMPDIR ?? process.env.TEMP ?? "/tmp"}/den-process-cwd-${process.pid}`;
await createDirAll(dir);
const expected = await canonicalize(dir);
process.chdir(dir);
assertEquals(await canonicalize(process.cwd()), expected);
process.chdir(original);
assertEquals(await canonicalize(process.cwd()), await canonicalize(original));
