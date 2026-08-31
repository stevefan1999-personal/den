import { assert, assertEquals } from "den:assert";
import { serve } from "den:http";

const server = serve({
  listen: { host: "127.0.0.1", port: 0 },
  async fetch(request) {
    const headers = { "access-control-allow-origin": "*", "content-type": "text/plain" };
    if (new URL(request.url).pathname === "/post") {
      return new Response(await request.text(), { headers });
    }
    return new Response("hello-xhr", {
      headers: { ...headers, "access-control-expose-headers": "x-echo", "x-echo": "yes" },
    });
  },
});
const getUrl = `${server.url}get`;
const postUrl = `${server.url}post`;
try {
  const get = await new Promise((resolve, reject) => {
    const xhr = new XMLHttpRequest();
    xhr.open("GET", getUrl);
    xhr.onload = () => resolve(xhr);
    xhr.onerror = () => reject(new Error("xhr error"));
    xhr.send();
  });
  const posted = await new Promise((resolve, reject) => {
    const xhr = new XMLHttpRequest();
    xhr.open("POST", postUrl);
    xhr.setRequestHeader("Content-Type", "text/plain");
    xhr.onload = () => resolve(xhr);
    xhr.onerror = () => reject(new Error("xhr error"));
    xhr.send("ping");
  });
  let syncThrew = false;
  let invalidHeaderThrew = false;
  try {
    const xhr = new XMLHttpRequest();
    xhr.open("GET", getUrl, false);
  } catch (error) {
    syncThrew = error instanceof TypeError;
  }
  try {
    const xhr = new XMLHttpRequest();
    xhr.open("GET", getUrl);
    xhr.setRequestHeader("x-test", "bad\0value");
  } catch (error) {
    invalidHeaderThrew = error instanceof DOMException && error.name === "SyntaxError";
  }
  assertEquals(get.status, 200);
  assertEquals(get.responseText, "hello-xhr");
  assertEquals(get.getResponseHeader("x-echo"), "yes");
  assertEquals(get.readyState, XMLHttpRequest.DONE);
  assertEquals(posted.responseText, "ping");
  assertEquals(get.responseXML, null);
  assert(get instanceof EventTarget);
  assert(syncThrew);
  assert(invalidHeaderThrew);
} finally {
  await server.close();
}
