import { assert, assertEquals } from "den:assert";
import { serve } from "den:http";

const server = serve({
  listen: { host: "127.0.0.1", port: 0 },
  fetch(request) {
    if (request.method === "OPTIONS") {
      return new Response(null, {
        headers: {
          "access-control-allow-origin": "*",
          "access-control-allow-methods": "GET",
          "access-control-allow-headers": "cache-control",
        },
      });
    }
    return new Response("event: custom\ndata: a\n\ndata: b\n\n", {
      headers: {
        "access-control-allow-origin": "*",
        "content-type": "text/event-stream",
      },
    });
  },
});
let relativeThrew = false;
try {
  new EventSource("/relative");
} catch (error) {
  relativeThrew = error.name === "SyntaxError";
}
const es = new EventSource(server.url);
try {
  const custom = new Promise((resolve) => es.addEventListener("custom", (e) => resolve(e)));
  const message = new Promise((resolve) => {
    es.onmessage = (e) => resolve(e);
  });
  await new Promise((resolve, reject) => {
    es.onopen = () => resolve();
    es.onerror = () => reject(new Error("eventsource error " + es.readyState));
  });
  const first = await custom;
  const second = await message;
  assert(relativeThrew);
  assertEquals(first.data, "a");
  assert(first instanceof MessageEvent);
  assertEquals(second.data, "b");
  assert(second.origin.startsWith("http://127.0.0.1"));
} finally {
  es.close();
  await server.close({ drainMs: 0 });
}
assertEquals(es.readyState, EventSource.CLOSED);
