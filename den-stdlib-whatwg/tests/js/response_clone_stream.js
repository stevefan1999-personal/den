import { assertEquals } from "den:assert";

// `new Response(stream)` parks an empty byte placeholder behind the stream;
// clone() must tee the stream rather than copy that placeholder.
const source = new ReadableStream({
  start(controller) {
    controller.enqueue(new TextEncoder().encode("abc"));
    controller.close();
  },
});
const original = new Response(source);
const copy = original.clone();

assertEquals(original.bodyUsed, false);
assertEquals(copy.bodyUsed, false);
assertEquals(await copy.text(), "abc");
assertEquals(await original.text(), "abc");
assertEquals(original.bodyUsed, true);
