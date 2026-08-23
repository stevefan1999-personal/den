// JS fetch() wrapper: construct a Request, flatten it, call the Rust native.
(function (natives, api) {
  const { Headers, Request } = api;
  const nativeFetch = natives.fetch;

  async function fetch(input, init) {
    const request = input instanceof Request && init === undefined
      ? input
      : new Request(input, init);
    if (request.signal && request.signal.aborted) {
      throw new DOMException("The operation was aborted.", "AbortError");
    }
    const headers = [];
    request.headers.forEach((value, name) => headers.push([name, value]));
    let body = null;
    if (request.method !== "GET" && request.method !== "HEAD") {
      const buffer = await request.arrayBuffer();
      if (buffer.byteLength > 0) body = new Uint8Array(buffer);
    }
    return nativeFetch(request.url, {
      method: request.method,
      headers,
      body,
      signal: request.signal,
    });
  }

  return { ...api, fetch, Headers, Request };
})
