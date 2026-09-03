// A source is pulled only while the consumer has demand: with a high-water
// mark of one chunk, exactly one chunk may sit buffered ahead of the reader.
// This is the pacing the deleted native-source unit test used to assert, kept
// here on the JS source path that shares the same `should_pull` decision.
import { assert, assertEquals } from "den:assert";

let pulls = 0;
let remaining = 3;
const paced = new ReadableStream({
  pull(controller) {
    pulls++;
    if (remaining-- > 0) controller.enqueue("x");
    else controller.close();
  },
}, { highWaterMark: 1 });

const reader = paced.getReader();
const first = await reader.read();
assertEquals(first.value, "x");
assert(pulls <= 2, `at most one chunk may be buffered ahead of the reader, saw ${pulls}`);

let text = first.value;
for (;;) {
  const { value, done } = await reader.read();
  if (done) break;
  text += value;
}
assertEquals(text, "xxx");
assertEquals(pulls, 4, "three chunks and one end-of-stream pull");
