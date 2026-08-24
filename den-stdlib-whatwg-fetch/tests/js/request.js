import { assertEquals } from "den:assert";
const request = new Request("http://127.0.0.1/post", {
  method: "post",
  headers: { "X-A": "b", "Content-Type": "text/plain" },
  body: "hello",
});
assertEquals(request.method, "POST");
assertEquals(request.headers.get("x-a"), "b");
assertEquals(request.url, "http://127.0.0.1/post");
