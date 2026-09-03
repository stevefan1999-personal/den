// A write parked on the transform's backpressure runs the transformer only
// once the reader pulls, parks again while the readable has no demand, and a
// write still parked when the stream errors rejects with the stored error.
import { assertEquals, assertStrictEquals } from "den:assert";

const outcome = async (promise) => {
  try {
    return ["fulfilled", await promise];
  } catch (error) {
    return ["rejected", error];
  }
};
const macrotask = () => new Promise((resolve) => setTimeout(resolve, 0));

// The default readable strategy has a high-water mark of zero, so every write
// waits for a pull.
const transformed = [];
const doubling = new TransformStream({
  transform(chunk, controller) {
    transformed.push(chunk);
    controller.enqueue(chunk * 2);
  },
});
const writer = doubling.writable.getWriter();
const reader = doubling.readable.getReader();

const first = writer.write(1);
await macrotask();
assertEquals(transformed, []);
assertEquals((await reader.read()).value, 2);
assertEquals(await outcome(first), ["fulfilled", undefined]);

const second = writer.write(2);
await macrotask();
assertEquals(transformed, [1]);
assertEquals((await reader.read()).value, 4);
assertEquals(await outcome(second), ["fulfilled", undefined]);
assertEquals(transformed, [1, 2]);

// Cancelling the readable errors the writable with the cancel reason; the
// parked write must reject with that reason rather than run the transformer.
const cancelled = new TransformStream({
  transform() {
    throw new Error("the transformer must not run");
  },
});
const cancelledWriter = cancelled.writable.getWriter();
const parked = cancelledWriter.write("parked");
await macrotask();
const reason = new Error("reader left");
assertEquals(await outcome(cancelled.readable.cancel(reason)), ["fulfilled", undefined]);
const [state, error] = await outcome(parked);
assertEquals(state, "rejected");
assertStrictEquals(error, reason);
