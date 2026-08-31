import { assertEquals } from "den:assert";
import { serve } from "den:http";

let retries = 0;
let rangeHeaders;
const headers = { "access-control-allow-origin": "*", "content-type": "text/plain" };
const server = serve({
  listen: { host: "127.0.0.1", port: 0 },
  async fetch(request) {
    const path = new URL(request.url).pathname;
    if (path === "/range") {
      rangeHeaders = [request.headers.get("range"), request.headers.get("accept-encoding")];
      return new Response("range", { headers });
    }
    const body = await request.text();
    if (path === "/retry") {
      return new Response(body, { status: ++retries === 1 ? 421 : 200, headers });
    }
    if (path === "/post") return new Response(body, { headers });
    return new Response('{"ok":true}', {
      headers: { ...headers, "content-type": "application/json" },
    });
  },
});

try {
  const get = await fetch(`${server.url}get`);
  const json = await get.json();
  const posted = await fetch(`${server.url}post`, {
    method: "POST",
    headers: { "Content-Type": "text/plain" },
    body: "ping",
  });
  const echoed = await posted.text();
  const ranged = await fetch(`${server.url}range`, { headers: { Range: "bytes=0-10" } });
  const retried = await fetch(`${server.url}retry`, { method: "POST", body: "retry" });
  assertEquals(get.status, 200);
  assertEquals(json.ok, true);
  assertEquals(posted.status, 200);
  assertEquals(echoed, "ping");
  assertEquals(get.type, "cors");
  assertEquals(posted.headers.get("content-type"), "text/plain");
  assertEquals(await ranged.text(), "range");
  assertEquals(retried.status, 200);
  assertEquals(await retried.text(), "retry");
  assertEquals(rangeHeaders, ["bytes=0-10", "identity"]);
  assertEquals(retries, 2);
} finally {
  await server.close();
}
