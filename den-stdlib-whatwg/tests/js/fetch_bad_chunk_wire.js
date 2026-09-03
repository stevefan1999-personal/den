import { assertEquals } from "den:assert";

// A malformed chunk on the wire fails the body with a TypeError after the
// good chunk was delivered; the URL text plays no part in it.
const response = await fetch(process.env.DEN_TEST_BAD_CHUNK_URL);
assertEquals(response.status, 200);
const reader = response.body.getReader();
const first = await reader.read();
assertEquals(new TextDecoder().decode(first.value), "hello");
let failure = "none";
try {
  await reader.read();
} catch (error) {
  failure = error.constructor.name;
}
assertEquals(failure, "TypeError");
let closed = "resolved";
try {
  await reader.closed;
} catch (error) {
  closed = error.constructor.name;
}
assertEquals(closed, "TypeError");
