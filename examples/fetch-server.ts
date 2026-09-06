/// <reference path="../types/den-http.d.ts" />

// cargo run -- examples/fetch-server.ts
//
// One process is both sides: den:http serve (Fetch handler) and fetch()
// against that listener. AbortController cancels an in-flight request.

import { assertEquals } from "den:assert";
import { serve } from "den:http";

let started!: () => void;
const requestStarted = new Promise<void>((resolve) => {
  started = resolve;
});

const cors = { "access-control-allow-origin": "*" };

const server = serve({
  listen: { host: "127.0.0.1", port: 0 },
  fetch(request) {
    const path = new URL(request.url).pathname;
    if (request.method === "OPTIONS") {
      return new Response(null, {
        status: 204,
        headers: {
          ...cors,
          "access-control-allow-methods": "*",
          "access-control-allow-headers": "*",
        },
      });
    }
    if (path === "/hang") {
      started();
      return new Promise<Response>(() => {});
    }
    if (path === "/echo") {
      return request.text().then((body) => new Response(body, { headers: cors }));
    }
    if (path === "/form") {
      return request.formData().then((form) =>
        Response.json({ runtime: String(form.get("runtime")) }, { headers: cors })
      );
    }
    return Response.json({
      runtime: "den",
      now: Temporal.Now.instant().toString(),
    }, { headers: cors });
  },
});

try {
  const json = await (await fetch(server.url)).json() as { runtime: string };
  assertEquals(json.runtime, "den");
  console.log("get", json.runtime, "at", server.url);

  const echoed = await (await fetch(`${server.url}echo`, {
    method: "POST",
    body: "ping",
  })).text();
  assertEquals(echoed, "ping");
  console.log("echo", echoed);

  const form = new FormData();
  form.append("runtime", "den");
  const posted = await (await fetch(`${server.url}form`, {
    method: "POST",
    body: form,
  })).json() as { runtime: string };
  assertEquals(posted.runtime, "den");
  console.log("form", posted.runtime);

  const controller = new AbortController();
  const pending = fetch(`${server.url}hang`, { signal: controller.signal });
  await requestStarted;
  controller.abort();
  let aborted = "not-aborted";
  try {
    await pending;
  } catch (error) {
    aborted = error instanceof Error ? error.name : String(error);
  }
  assertEquals(aborted, "AbortError");
  console.log("abort", aborted);
} finally {
  await server.close({ drainMs: 0 });
}
