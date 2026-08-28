import { assert, assertEquals } from "den:assert";

const encoder = new TextEncoder();
const decoder = new TextDecoder();
const input = encoder.encode("a note for den");

const collect = async (stream, chunk) => {
  const writer = stream.writable.getWriter();
  const reader = stream.readable.getReader();
  writer.write(chunk);
  writer.close();
  const parts = [];
  for (;;) {
    const { value, done } = await reader.read();
    if (done) break;
    parts.push(value);
  }
  const total = parts.reduce((sum, part) => sum + part.length, 0);
  const out = new Uint8Array(total);
  let offset = 0;
  for (const part of parts) {
    out.set(part, offset);
    offset += part.length;
  }
  return out;
};

const compressed = await collect(new CompressionStream("gzip"), input);
assert(compressed.length > 0);
assertEquals(compressed[0], 0x1f);
assertEquals(compressed[1], 0x8b);

const plain = await collect(new DecompressionStream("gzip"), compressed);
assertEquals(decoder.decode(plain), "a note for den");
