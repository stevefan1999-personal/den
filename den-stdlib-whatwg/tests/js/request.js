import { assertEquals, assertThrows } from "den:assert";
assertEquals(Request.length, 1);
assertEquals(Request.prototype.constructor, Request);
assertThrows(() => Request("http://127.0.0.1/"), TypeError);
const request = new Request("http://127.0.0.1/post", {
  method: "post",
  headers: { "X-A": "b", "Content-Type": "text/plain" },
  body: "hello",
});
assertEquals(request.method, "POST");
assertEquals(request.headers.get("x-a"), "b");
assertEquals(request.url, "http://127.0.0.1/post");
