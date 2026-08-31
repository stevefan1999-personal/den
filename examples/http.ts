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

export function route(path: string, pattern: string): boolean {
  return new URLPattern({ pathname: pattern }).test(`http://local${path}`);
}

export async function fromRequest(request: Request): Promise<HttpRequest> {
  const headers = new Map<string, string>();
  request.headers.forEach((value, name) => headers.set(name, value));
  return {
    method: request.method,
    path: new URL(request.url).pathname,
    headers,
    body: await request.text(),
  };
}

export function toResponse(reply: HttpReply): Response {
  const headers = new Headers({
    "access-control-allow-origin": "*",
    "access-control-allow-methods": "*",
    "access-control-allow-headers": "*",
    "access-control-expose-headers": "*",
    "content-type": reply.type,
  });
  for (const line of reply.extra ?? []) {
    const colon = line.indexOf(":");
    if (colon > 0) {
      headers.append(line.slice(0, colon), line.slice(colon + 1).trim());
    }
  }
  const noBody = reply.status === 204 || reply.status === 205 ||
    reply.status === 304;
  return new Response(noBody ? null : reply.body, {
    status: reply.status,
    statusText: reply.reason,
    headers,
  });
}
