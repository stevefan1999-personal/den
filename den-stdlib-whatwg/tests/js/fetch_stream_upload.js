import { assertEquals } from "den:assert";
import { serve } from "den:http";

let framing;
let requests = 0;
const server = serve({
  listen: { host: "127.0.0.1", port: 0 },
  async fetch(request) {
    requests++;
    framing = request.headers.get("transfer-encoding");
    return new Response(await request.text(), {
      status: 421,
      headers: { "access-control-allow-origin": "*", "content-type": "text/plain" },
    });
  },
});

// The body is produced lazily: `pull` is only asked for the next chunk once
// the transport has taken the previous one, so a request body that is still
// being generated is already on the wire.
const encoder = new TextEncoder();
const parts = ["strea", "med-", "upload"];
let pulled = 0;
const body = new ReadableStream({
  pull(controller) {
    if (pulled === parts.length) {
      controller.close();
      return;
    }
    controller.enqueue(encoder.encode(parts[pulled++]));
  },
});

try {
  const echoed = await fetch(`${server.url}upload`, { method: "POST", body, duplex: "half" });
  assertEquals(await echoed.text(), "streamed-upload");
  assertEquals(pulled, parts.length);
  assertEquals(framing, "chunked");
  assertEquals(requests, 1);

  // Without `duplex` the request is not the one the caller asked for.
  let refused = null;
  try {
    new Request(`${server.url}upload`, {
      method: "POST",
      body: new ReadableStream(),
    });
  } catch (error) {
    refused = error;
  }
  assertEquals(refused instanceof TypeError, true);
} finally {
  await server.close();
}
