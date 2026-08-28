import { type ByteStream } from "./net.ts";

export interface HttpRequest {
  method: string;
  path: string;
  headers: Map<string, string>;
  body: string;
}

export interface HttpReply {
  status: number;
  reason: string;
  type: string;
  body: string;
  extra?: string[];
}

const CORS = [
  "Access-Control-Allow-Origin: *",
  "Access-Control-Allow-Methods: *",
  "Access-Control-Allow-Headers: *",
  "Access-Control-Expose-Headers: *",
];

function concat(left: Uint8Array, right: Uint8Array): Uint8Array {
  const out = new Uint8Array(left.length + right.length);
  out.set(left);
  out.set(right, left.length);
  return out;
}

export function route(path: string, pattern: string): boolean {
  return new URLPattern({ pathname: pattern }).test(`http://local${path}`);
}

export async function readRequest(stream: ByteStream): Promise<HttpRequest> {
  const decoder = new TextDecoder();
  let raw = new Uint8Array(0);
  for (;;) {
    const chunk = await stream.read(4096);
    if (chunk.length === 0) {
      throw new Error("empty request");
    }
    raw = concat(raw, chunk);
    const text = decoder.decode(raw);
    const split = text.indexOf("\r\n\r\n");
    if (split === -1) {
      continue;
    }
    const lines = text.slice(0, split).split("\r\n");
    const [method, target] = lines[0].split(" ");
    const headers = new Map<string, string>();
    for (const line of lines.slice(1)) {
      const colon = line.indexOf(":");
      if (colon !== -1) {
        headers.set(line.slice(0, colon).toLowerCase(), line.slice(colon + 1).trim());
      }
    }
    const need = Number(headers.get("content-length") ?? 0);
    let body = raw.subarray(split + 4);
    while (body.length < need) {
      const more = await stream.read(need - body.length);
      if (more.length === 0) {
        break;
      }
      body = concat(body, more);
    }
    const path = (target ?? "/").split("?")[0] ?? "/";
    return {
      method: method ?? "GET",
      path,
      headers,
      body: decoder.decode(body.subarray(0, need)),
    };
  }
}

export function encode(reply: HttpReply): Uint8Array {
  const bytes = new TextEncoder().encode(reply.body);
  const head = [
    `HTTP/1.1 ${reply.status} ${reply.reason}`,
    `Content-Type: ${reply.type}`,
    `Content-Length: ${bytes.length}`,
    ...CORS,
    ...(reply.extra ?? []),
    "Connection: close",
    "",
    "",
  ].join("\r\n");
  return concat(new TextEncoder().encode(head), bytes);
}
