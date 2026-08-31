import { assert, assertEquals, assertThrows } from "den:assert";

const error = Response.error();
assertEquals(error.type, "error");
assertEquals(error.status, 0);
assertEquals(error.bodyUsed, false);

const redirected = Response.redirect("https://example.test/next", 307);
assertEquals(redirected.status, 307);
assertEquals(redirected.headers.get("location"), "https://example.test/next");
assertThrows(() => Response.redirect("https://example.test/next", 200), RangeError);

const json = Response.json({ ok: true, n: 2 });
assertEquals(json.headers.get("content-type").startsWith("application/json"), true);
assertEquals((await json.json()).n, 2);

assertThrows(() => new Response("x", { status: 99 }), RangeError);
assertThrows(() => new Response("x", { statusText: "no\npe" }), TypeError);
assertThrows(() => new Response("x", { status: 204 }), TypeError);

const params = new URLSearchParams({ a: "1" });
const encoded = new Response(params);
assertEquals(encoded.headers.get("content-type"), "application/x-www-form-urlencoded;charset=UTF-8");
assertEquals(await encoded.text(), "a=1");

const emptyForm = new FormData();
const formResponse = new Response(emptyForm);
assert(formResponse.headers.get("content-type").startsWith("multipart/form-data"));

const bytes = new Response(Uint8Array.of(1, 2, 3));
assertEquals([...(await bytes.bytes())].join(","), "1,2,3");

let blocked = false;
try {
  await fetch("http://127.0.0.1:25/");
} catch {
  blocked = true;
}
assert(blocked);
