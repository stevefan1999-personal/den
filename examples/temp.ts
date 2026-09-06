import { posix } from "den:path";
import { env } from "den:process";

export function tempDir(): string {
  return env.TMPDIR ?? env.TEMP ?? "/tmp";
}

export function uniquePath(prefix: string, suffix = ""): string {
  return posix.join(tempDir(), `${prefix}-${crypto.randomUUID()}${suffix}`);
}

export function utf8Bytes(text: string): number[] {
  return Array.from(new TextEncoder().encode(text));
}
