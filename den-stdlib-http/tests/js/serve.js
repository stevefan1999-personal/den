import { HttpError, serve } from "den:http";
import { TcpStream } from "den:networking";

let connection;
let releaseSlow;
let markSlowStarted;
const slowStarted = new Promise((resolve) => {
  markSlowStarted = resolve;
});
async function raw(server, request) {
  const socket = await TcpStream.connect(`${server.addr.hostname}:${server.addr.port}`);
  const write = socket.writeAll ?? socket.write_all;
  const read = socket.readToString ?? socket.read_to_string;
  await write.call(socket, request);
  return read.call(socket);
}

const server = serve({
  listen: { host: "127.0.0.1", port: 0 },
  fetch(request, info) {
    const path = request.url.slice(request.url.indexOf("/", 7));
    if (path === "/throw") {
      throw new Error("handler secret must stay off the wire");
    }
    if (path === "/bad-length") {
      return new Response("hello", {
        headers: { "content-length": "2", "transfer-encoding": "chunked" },
      });
    }
    if (path === "/signal") {
      return new Response(request.signal instanceof AbortSignal ? "signal" : "missing");
    }
    if (path === "/huge-response") {
      return new Response(new Uint8Array(16 * 1024 * 1024 + 1));
    }
    if (path === "/stream-response") {
      return new Response(new ReadableStream({ start(controller) { controller.enqueue(new Uint8Array(1)); } }));
    }
    connection = info;
    if (path === "/inspect") {
      return new Response(`${request.url}|${request.headers.get("host")}`);
    }
    if (path === "/slow") {
      markSlowStarted();
      return new Promise((resolve) => {
        releaseSlow = () =>
          resolve(
            new Response("drained", {
              headers: { "access-control-allow-origin": "*" },
            }),
          );
      });
    }
    return request.text().then(
      (text) =>
        new Response(`${request.method} ${path} ${text}`, {
          headers: { "access-control-allow-origin": "*" },
        }),
    );
  },
});

const response = await fetch(`${server.url}echo`, { method: "POST", body: "hello" });
const body = await response.text();
const badTarget = await raw(
  server,
  "GET http://attacker.invalid/ HTTP/1.1\r\nHost: attacker.invalid\r\nConnection: close\r\n\r\n",
);
const badGetBody = await raw(
  server,
  "GET / HTTP/1.1\r\nHost: attacker.invalid\r\nContent-Length: 1\r\nConnection: close\r\n\r\nx",
);
const methodMiss = await raw(
  server,
  "TRACE / HTTP/1.1\r\nHost: attacker.invalid\r\nConnection: close\r\n\r\n",
);
const head = await raw(
  server,
  "HEAD /echo HTTP/1.1\r\nHost: attacker.invalid\r\nConnection: close\r\n\r\n",
);
const handlerFailure = await raw(
  server,
  "GET /throw HTTP/1.1\r\nHost: attacker.invalid\r\nConnection: close\r\n\r\n",
);
const authority = await raw(
  server,
  "GET /inspect HTTP/1.1\r\nHost: attacker.invalid\r\nConnection: close\r\n\r\n",
);
const h2Authority = await __denH2Fetch(`${server.url}inspect`);
const framing = await raw(
  server,
  "GET /bad-length HTTP/1.1\r\nHost: attacker.invalid\r\nConnection: close\r\n\r\n",
);
const signal = await raw(
  server,
  "GET /signal HTTP/1.1\r\nHost: attacker.invalid\r\nConnection: close\r\n\r\n",
);
const oversizedRequest = await raw(
  server,
  "POST /echo HTTP/1.1\r\nHost: attacker.invalid\r\nContent-Length: 16777217\r\nConnection: close\r\n\r\n",
);
const oversizedResponse = await raw(
  server,
  "GET /huge-response HTTP/1.1\r\nHost: attacker.invalid\r\nConnection: close\r\n\r\n",
);
const streamingResponse = await raw(
  server,
  "GET /stream-response HTTP/1.1\r\nHost: attacker.invalid\r\nConnection: close\r\n\r\n",
);
let bindError = false;
try {
  serve({
    listen: { host: "127.0.0.1", port: server.addr.port },
    fetch: () => new Response(),
  });
} catch (error) {
  bindError = error instanceof HttpError && error instanceof Error && error.kind === "AddrInUse";
}
let invalidDrain = false;
try {
  server.close({ drainMs: -1 });
} catch (error) {
  invalidDrain = error instanceof RangeError;
}

let markHungStarted;
let hungSignal;
const hungStarted = new Promise((resolve) => {
  markHungStarted = resolve;
});
const deadlineServer = serve({
  listen: { host: "127.0.0.1", port: 0 },
  fetch(request) {
    hungSignal = request.signal;
    markHungStarted();
    return new Promise(() => {});
  },
});
const hungFetch = fetch(deadlineServer.url).then(
  () => false,
  () => true,
);
await hungStarted;
await deadlineServer.close({ drainMs: 0 });
await deadlineServer.finished;
const forcedDrain =
  (await hungFetch) &&
  hungSignal instanceof AbortSignal &&
  hungSignal.aborted;

const slowRequest = fetch(`${server.url}slow`);
await slowStarted;
const finished = server.finished;
const port = server.addr.port;
const asyncDispose = server[Symbol.asyncDispose] === server.close;
const closing = server.close({ drainMs: 1_000 });
releaseSlow();
const slowBody = await (await slowRequest).text();
await closing;
await finished;

globalThis.__httpResult = JSON.stringify({
  body,
  port,
  url: server.url,
  remote: connection.remote.port > 0,
  local: connection.local.port === port,
  pending: server.pending,
  bindError,
  invalidDrain,
  forcedDrain,
  asyncDispose,
  slowBody,
  badTarget: badTarget.startsWith("HTTP/1.1 400") && badTarget.endsWith("Bad Request\n"),
  badGetBody: badGetBody.startsWith("HTTP/1.1 400") && badGetBody.endsWith("Bad Request\n"),
  methodMiss:
    methodMiss.startsWith("HTTP/1.1 405") &&
    methodMiss.includes("allow: GET, HEAD, POST, PUT, PATCH, DELETE, OPTIONS") &&
    methodMiss.endsWith("Method Not Allowed\n"),
  head:
    head.startsWith("HTTP/1.1 200") &&
    head.includes("content-length: 11") &&
    head.endsWith("\r\n\r\n") &&
    !head.includes("HEAD /echo"),
  handlerFailure:
    handlerFailure.startsWith("HTTP/1.1 500") &&
    handlerFailure.endsWith("Internal Server Error\n") &&
    !handlerFailure.includes("handler secret"),
  authority:
    authority.endsWith(`${server.url}inspect|attacker.invalid`) &&
    !authority.includes("http://attacker.invalid/inspect"),
  http2:
    h2Authority ===
    `${server.url}inspect|${server.addr.hostname}:${server.addr.port}`,
  framing:
    framing.startsWith("HTTP/1.1 200") &&
    framing.includes("content-length: 5") &&
    !framing.includes("transfer-encoding") &&
    framing.endsWith("hello"),
  signal: signal.endsWith("signal"),
  oversizedRequest: oversizedRequest.startsWith("HTTP/1.1 413"),
  oversizedResponse: oversizedResponse.startsWith("HTTP/1.1 500"),
  streamingResponse: streamingResponse.startsWith("HTTP/1.1 500"),
  importOnly: globalThis.serve === undefined,
});
