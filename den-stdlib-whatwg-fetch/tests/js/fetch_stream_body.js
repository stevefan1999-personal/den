import { assertEquals } from "den:assert";

// A cacheable response is streamed, not pre-buffered: the reader sees the
// bytes, and the HTTP cache is filled from what flowed past rather than from
// a copy taken before the body was handed over.
const first = await fetch(process.env.DEN_TEST_CACHED_URL);
const reader = first.body.getReader();
const chunks = [];
for (let next = await reader.read(); !next.done; next = await reader.read()) {
  chunks.push(...next.value);
}
assertEquals(new TextDecoder().decode(new Uint8Array(chunks)), "streamed");

const second = await fetch(process.env.DEN_TEST_CACHED_URL);
assertEquals(await second.text(), "streamed");
