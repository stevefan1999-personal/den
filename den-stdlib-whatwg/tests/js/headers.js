import { assertEquals } from "den:assert";
const headers = new Headers({ "X-A": "b" });
headers.append("X-A", "c");
const request = new Request("http://127.0.0.1/post", {
  method: "post",
  headers,
  body: "hi",
});
assertEquals(headers.get("x-a"), "b, c");
assertEquals(request.method, "POST");
assertEquals(request.url, "http://127.0.0.1/post");
assertEquals(typeof fetch, "function");
