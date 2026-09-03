import { assertEquals } from "den:assert";
import { serve } from "den:http";

// "bad-chunk" is the name of a WPT fixture script, not a protocol signal: a URL
// that happens to contain it is an ordinary URL and must be fetched like one,
// on the buffered path and on the streaming one.
const server = serve({
  listen: { host: "127.0.0.1", port: 0 },
  fetch: () =>
    new Response("intact", {
      headers: { "access-control-allow-origin": "*", "content-type": "text/plain" },
    }),
});

try {
  assertEquals(await (await fetch(`${server.url}bad-chunk`)).text(), "intact");

  const streamed = await fetch(`${server.url}bad-chunk-encoding.py`);
  const reader = streamed.body.getReader();
  const decoder = new TextDecoder();
  let text = "";
  for (;;) {
    const { done, value } = await reader.read();
    if (done) break;
    text += decoder.decode(value, { stream: true });
  }
  assertEquals(text, "intact");
} finally {
  await server.close();
}
