// cargo run -- examples/compress.ts
//
// WinterTC CompressionStream / DecompressionStream: gzip, deflate, deflate-raw.

export {};

async function collect(
  stream: CompressionStream | DecompressionStream,
  chunk: Uint8Array,
): Promise<Uint8Array> {
  const writer = stream.writable.getWriter();
  const reader = stream.readable.getReader();
  writer.write(chunk);
  writer.close();
  const parts: Uint8Array[] = [];
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
}

const encoder = new TextEncoder();
const decoder = new TextDecoder();
const input = encoder.encode("a note for den");
const compressed = await collect(new CompressionStream("gzip"), input);
const plain = await collect(new DecompressionStream("gzip"), compressed);
console.log("gzip", compressed.length, "bytes, magic", compressed[0], compressed[1]);
console.log("round-trip", decoder.decode(plain));
