import { assert, assertEquals } from "den:assert";

const encoder = new TextEncoder();
const decoder = new TextDecoder();
const input = encoder.encode("Hello, WinterTC compression streams.");
const roundTrip = async (format) => {
  const cs = new CompressionStream(format);
  const writer = cs.writable.getWriter();
  const reader = cs.readable.getReader();
  writer.write(input);
  writer.close();
  const chunks = [];
  for (;;) {
    const { value, done } = await reader.read();
    if (done) break;
    chunks.push(value);
  }
  const total = chunks.reduce((sum, chunk) => sum + chunk.length, 0);
  const compressed = new Uint8Array(total);
  let offset = 0;
  for (const chunk of chunks) {
    compressed.set(chunk, offset);
    offset += chunk.length;
  }
  const ds = new DecompressionStream(format);
  const dwriter = ds.writable.getWriter();
  const dreader = ds.readable.getReader();
  dwriter.write(compressed);
  dwriter.close();
  const out = [];
  for (;;) {
    const { value, done } = await dreader.read();
    if (done) break;
    out.push(value);
  }
  const outTotal = out.reduce((sum, chunk) => sum + chunk.length, 0);
  const plain = new Uint8Array(outTotal);
  offset = 0;
  for (const chunk of out) {
    plain.set(chunk, offset);
    offset += chunk.length;
  }
  return {
    empty: compressed.length === 0,
    gzipMagic: format !== "gzip" || (compressed[0] === 0x1f && compressed[1] === 0x8b),
    text: decoder.decode(plain),
  };
};
const gzip = await roundTrip("gzip");
const deflate = await roundTrip("deflate");
const raw = await roundTrip("deflate-raw");
let invalid = false;
try {
  new CompressionStream("nope");
} catch (error) {
  invalid = error instanceof TypeError;
}
assertEquals(gzip.text, "Hello, WinterTC compression streams.");
assertEquals(deflate.text, "Hello, WinterTC compression streams.");
assertEquals(raw.text, "Hello, WinterTC compression streams.");
assert(gzip.gzipMagic);
assert(!gzip.empty);
assert(invalid);
