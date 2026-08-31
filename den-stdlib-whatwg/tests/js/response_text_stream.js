import { assertEquals } from "den:assert";

// Decoding a buffered body used to hold a read borrow of it across the write
// that marks it used, which aborted the process.
const response = new Response("hi");
const reader = response.textStream().getReader();
assertEquals(response.bodyUsed, true);
let text = "";
for (let next = await reader.read(); !next.done; next = await reader.read()) {
  text += next.value;
}
assertEquals(text, "hi");

// A null body is never used, however it is asked for.
const empty = new Response();
empty.textStream();
assertEquals(empty.bodyUsed, false);
