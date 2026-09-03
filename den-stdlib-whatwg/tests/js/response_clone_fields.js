import { assertEquals } from "den:assert";

// clone() copies every field, not just the body, whichever kind of body the
// response holds, and leaves the original untouched.
const fields = (response) => [
  response.status, response.statusText, response.type, response.url, response.redirected,
  response.headers.get("x-a"), response.bodyUsed,
];
const buffered = new Response("body", { status: 201, statusText: "Made", headers: { "x-a": "1" } });
const copy = buffered.clone();
assertEquals(fields(copy), fields(buffered));
copy.headers.set("x-a", "2");
assertEquals(buffered.headers.get("x-a"), "1");
assertEquals([await copy.text(), await buffered.text()], ["body", "body"]);

const empty = new Response(null, { status: 204, headers: { "x-a": "e" } });
assertEquals(fields(empty.clone()), fields(empty));
assertEquals(await empty.clone().text(), "");

const redirect = Response.redirect("https://example.test/next", 301).clone();
assertEquals([redirect.status, redirect.headers.get("location")], [301, "https://example.test/next"]);
assertEquals(Response.error().clone().type, "error");

const streamed = new Response(
  new ReadableStream({
    start(controller) {
      controller.enqueue(new TextEncoder().encode("s"));
      controller.close();
    },
  }),
  { status: 202, statusText: "S" },
);
const teed = streamed.clone();
assertEquals(fields(teed), fields(streamed));
assertEquals([await teed.text(), await streamed.text()], ["s", "s"]);
let thrown;
try {
  buffered.clone();
} catch (error) {
  thrown = `${error.constructor.name}:${error.message}`;
}
assertEquals(thrown, "TypeError:Already read");
