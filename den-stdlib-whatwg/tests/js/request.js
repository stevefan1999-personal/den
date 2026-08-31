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

assertEquals(new Request("http://127.0.0.1/", { keepalive: 0n }).keepalive, false);
assertThrows(
  () => new Request("http://127.0.0.1/", { credentials: null }),
  TypeError,
);
assertThrows(() => new Request("http://127.0.0.1/", 1), TypeError);

assertEquals(new Response(null, { status: "204" }).status, 204);
assertEquals(new Response(null, { statusText: null }).statusText, "null");
const statusError = new Error("status getter");
let caughtStatusError;
try {
  new Response(null, Object.defineProperty({}, "status", {
    get() { throw statusError; },
  }));
} catch (error) {
  caughtStatusError = error;
}
assertEquals(caughtStatusError, statusError);
