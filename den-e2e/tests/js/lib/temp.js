import { posix } from "den:path";

export function tempDir(name) {
  const root = process.env.TMPDIR ?? process.env.TEMP ?? "/tmp";
  return posix.join(root, `den-e2e-${process.pid}-${name}`);
}
