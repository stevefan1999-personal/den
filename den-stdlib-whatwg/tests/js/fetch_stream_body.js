import { assertEquals } from "den:assert";
import { serve } from "den:http";

// A cacheable response is streamed, not pre-buffered: the reader sees the
// bytes, and the HTTP cache is filled from what flowed past rather than from
// a copy taken before the body was handed over.
let hits = 0;
const server = serve({
  listen: { host: "127.0.0.1", port: 0 },
  fetch() {
    hits++;
    return new Response("streamed", {
      headers: { "access-control-allow-origin": "*", "cache-control": "max-age=60" },
    });
  },
});
try {
  const first = await fetch(`${server.url}cached`);
  const reader = first.body.getReader();
  const chunks = [];
  for (let next = await reader.read(); !next.done; next = await reader.read()) {
    chunks.push(...next.value);
  }
  assertEquals(new TextDecoder().decode(new Uint8Array(chunks)), "streamed");

  const second = await fetch(`${server.url}cached`);
  assertEquals(await second.text(), "streamed");
  assertEquals(hits, 1);
} finally {
  await server.close();
}
