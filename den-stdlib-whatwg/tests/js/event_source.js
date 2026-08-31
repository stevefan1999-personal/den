import { assert, assertEquals } from "den:assert";
import { serve } from "den:http";

assertEquals(EventSource.CONNECTING, 0);
assertEquals(EventSource.OPEN, 1);
assertEquals(EventSource.CLOSED, 2);

const server = serve({
  listen: { host: "127.0.0.1", port: 0 },
  fetch(request) {
    if (request.method === "OPTIONS") {
      return new Response(null, {
        headers: {
          "access-control-allow-origin": "*",
          "access-control-allow-methods": "GET",
          "access-control-allow-headers": "cache-control, last-event-id",
        },
      });
    }
    const url = new URL(request.url);
    if (url.pathname.endsWith("/wrong-mime")) {
      return new Response("nope", {
        headers: { "access-control-allow-origin": "*", "content-type": "text/plain" },
      });
    }
    const last = request.headers.get("last-event-id") ?? "";
    return new Response(
      `: keep-alive\r\nid: 7\nevent: custom\ndata: a\n\ndata: b\n\nretry: 50\nid: 8\ndata: ${last || "first"}\n\n`,
      {
        headers: {
          "access-control-allow-origin": "*",
          "content-type": "text/event-stream",
        },
      },
    );
  },
});
let relativeThrew = false;
try {
  new EventSource("/relative");
} catch (error) {
  relativeThrew = error.name === "SyntaxError";
}
const es = new EventSource(server.url, { withCredentials: false });
const wrong = new EventSource(`${server.url}wrong-mime`);
try {
  const custom = new Promise((resolve) => es.addEventListener("custom", (e) => resolve(e)));
  const message = new Promise((resolve) => {
    es.onmessage = (e) => resolve(e);
  });
  const wrongFailed = new Promise((resolve) => {
    wrong.onerror = () => resolve(wrong.readyState);
  });
  await new Promise((resolve, reject) => {
    es.onopen = () => resolve();
    es.onerror = () => reject(new Error("eventsource error " + es.readyState));
  });
  const first = await custom;
  const second = await message;
  assert(relativeThrew);
  assertEquals(es.withCredentials, false);
  assertEquals(first.data, "a");
  assert(first instanceof MessageEvent);
  assertEquals(first.lastEventId, "7");
  assertEquals(second.data, "b");
  assert(second.origin.startsWith("http://127.0.0.1"));
  assertEquals(await wrongFailed, EventSource.CLOSED);
} finally {
  es.close();
  wrong.close();
  await server.close({ drainMs: 0 });
}
assertEquals(es.readyState, EventSource.CLOSED);
