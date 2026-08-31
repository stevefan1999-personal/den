import { assert, assertEquals } from "den:assert";
import { serve } from "den:http";

const server = serve({
  listen: { host: "127.0.0.1", port: 0 },
  async fetch(request) {
    const url = new URL(request.url);
    const headers = {
      "access-control-allow-origin": "*",
      "content-type": "text/plain; charset=utf-8",
    };
    if (url.pathname === "/post") {
      return new Response(await request.text(), { headers });
    }
    if (url.pathname === "/json") {
      return new Response(JSON.stringify({ ok: true, n: 1 }), {
        headers: { ...headers, "content-type": "application/json", "x-echo": "a", "X-Echo": "b" },
      });
    }
    if (url.pathname === "/slow") {
      await new Promise((resolve) => setTimeout(resolve, 50));
      return new Response("late", { headers });
    }
    if (url.pathname === "/latin1") {
      return new Response(Uint8Array.of(0xc9), {
        headers: { ...headers, "content-type": "text/plain; charset=iso-8859-1" },
      });
    }
    return new Response("hello-xhr", {
      headers: { ...headers, "access-control-expose-headers": "x-echo", "x-echo": "yes" },
    });
  },
});
const getUrl = `${server.url}get`;
const postUrl = `${server.url}post`;
const jsonUrl = `${server.url}json`;
const slowUrl = `${server.url}slow`;
const latin1Url = `${server.url}latin1`;

function send(method, url, { body, type, headers, timeout, mime } = {}) {
  return new Promise((resolve, reject) => {
    const xhr = new XMLHttpRequest();
    xhr.open(method, url);
    if (type) xhr.responseType = type;
    if (timeout) xhr.timeout = timeout;
    if (mime) xhr.overrideMimeType(mime);
    for (const [name, value] of Object.entries(headers ?? {})) {
      xhr.setRequestHeader(name, value);
    }
    xhr.onload = () => resolve(xhr);
    xhr.onerror = () => reject(new Error("xhr error " + xhr.status));
    xhr.ontimeout = () => reject(new Error("xhr timeout"));
    xhr.send(body);
  });
}

try {
  assertEquals(XMLHttpRequest.UNSENT, 0);
  assertEquals(XMLHttpRequest.OPENED, 1);
  assertEquals(XMLHttpRequest.HEADERS_RECEIVED, 2);
  assertEquals(XMLHttpRequest.LOADING, 3);
  assertEquals(XMLHttpRequest.DONE, 4);
  assertEquals(typeof new XMLHttpRequest().upload, "undefined");

  const get = await send("GET", getUrl);
  const posted = await send("POST", postUrl, {
    body: "ping",
    headers: { "Content-Type": "text/plain" },
  });
  const json = await send("GET", jsonUrl, { type: "json" });
  const buffer = await send("GET", getUrl, { type: "arraybuffer" });
  const latin1 = await send("GET", latin1Url, { mime: "text/plain; charset=utf-8" });

  let syncThrew = false;
  try {
    const xhr = new XMLHttpRequest();
    xhr.open("GET", getUrl, false);
  } catch (error) {
    syncThrew = error instanceof TypeError;
  }
  let invalidHeaderThrew = false;
  try {
    const xhr = new XMLHttpRequest();
    xhr.open("GET", getUrl);
    xhr.setRequestHeader("x-test", "bad\0value");
  } catch (error) {
    invalidHeaderThrew = error instanceof DOMException && error.name === "SyntaxError";
  }
  let headerStateThrew = false;
  try {
    const xhr = new XMLHttpRequest();
    xhr.setRequestHeader("x-test", "1");
  } catch (error) {
    headerStateThrew = error instanceof DOMException && error.name === "InvalidStateError";
  }
  let sendStateThrew = false;
  try {
    new XMLHttpRequest().send();
  } catch (error) {
    sendStateThrew = error instanceof DOMException && error.name === "InvalidStateError";
  }
  let responseTextTypeThrew = false;
  try {
    const xhr = new XMLHttpRequest();
    xhr.open("GET", getUrl);
    xhr.responseType = "json";
    xhr.responseText;
  } catch (error) {
    responseTextTypeThrew = error instanceof DOMException && error.name === "InvalidStateError";
  }

  const aborted = new XMLHttpRequest();
  aborted.open("GET", slowUrl);
  aborted.send();
  aborted.abort();

  const timed = new XMLHttpRequest();
  timed.timeout = Number.NaN;
  assertEquals(timed.timeout, 0);
  timed.timeout = 1500;
  assertEquals(timed.timeout, 1500);

  assertEquals(get.status, 200);
  assertEquals(get.responseText, "hello-xhr");
  assertEquals(get.getResponseHeader("x-echo"), "yes");
  assertEquals(get.readyState, XMLHttpRequest.DONE);
  assertEquals(posted.responseText, "ping");
  assertEquals(get.responseXML, null);
  assert(get instanceof EventTarget);
  assert(syncThrew);
  assert(invalidHeaderThrew);
  assert(headerStateThrew);
  assert(sendStateThrew);
  assert(responseTextTypeThrew);
  assertEquals(json.response.ok, true);
  assert(buffer.response instanceof ArrayBuffer);
  assertEquals(new TextDecoder().decode(buffer.response), "hello-xhr");
  assertEquals(typeof latin1.responseText, "string");
  assertEquals(typeof aborted.abort, "function");
} finally {
  await server.close();
}
