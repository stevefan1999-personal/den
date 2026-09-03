// Pins for the reader and writer acquisition path, the writer's promise
// getters, the ready/closed replacement on release and on error, the per-realm
// strategy `size`, and disturbed-stream detection.
import { assert, assertEquals, assertInstanceOf, assertStrictEquals, assertThrows } from "den:assert";

const outcome = async (promise) => {
  try {
    return ["fulfilled", await promise];
  } catch (error) {
    return ["rejected", error];
  }
};

// A second reader or writer is a TypeError carrying the lock message, whether
// acquired through the stream or through the constructor directly.
const readable = new ReadableStream();
const reader = readable.getReader();
assertThrows(() => readable.getReader(), TypeError, "ReadableStream is locked");
assertThrows(() => new ReadableStreamDefaultReader(readable), TypeError, "ReadableStream is locked");
assertThrows(() => new ReadableStreamDefaultReader({}), TypeError, "a ReadableStream is required");
reader.releaseLock();
assert(!readable.locked, "releaseLock unlocks the stream");

const writable = new WritableStream();
const writer = writable.getWriter();
assertThrows(() => writable.getWriter(), TypeError, "WritableStream is locked");
assertThrows(() => new WritableStreamDefaultWriter(writable), TypeError, "WritableStream is locked");
assertThrows(() => new WritableStreamDefaultWriter({}), TypeError, "a WritableStream is required");

// ready and closed are prototype accessors that hand back the same promise
// while the writer holds the lock.
for (const name of ["ready", "closed"]) {
  const descriptor = Object.getOwnPropertyDescriptor(WritableStreamDefaultWriter.prototype, name);
  assertEquals(typeof descriptor.get, "function");
  assertEquals(descriptor.set, undefined);
  assertStrictEquals(writer[name], writer[name]);
}
assertEquals(await writer.ready, undefined);

// Releasing a writer whose ready already settled replaces both promises with
// rejections carrying the release reason.
writer.releaseLock();
for (const name of ["ready", "closed"]) {
  const [state, reason] = await outcome(writer[name]);
  assertEquals(state, "rejected");
  assertInstanceOf(reason, TypeError);
  assertEquals(reason.message, "the writer was released");
}

// Erroring a stream whose writer.ready already fulfilled swaps in a rejection
// with the error; closed follows once erroring finishes.
const failure = new Error("sink gave up");
let erroringController;
const erroring = new WritableStream({
  start(controller) {
    erroringController = controller;
  },
});
const erroringWriter = erroring.getWriter();
await erroringWriter.ready;
erroringController.error(failure);
for (const name of ["ready", "closed"]) {
  const [state, reason] = await outcome(erroringWriter[name]);
  assertEquals(state, "rejected");
  assertStrictEquals(reason, failure);
}

// The two built-in strategies hand out one `size` per realm, named `size`.
assertStrictEquals(
  new CountQueuingStrategy({ highWaterMark: 1 }).size,
  new CountQueuingStrategy({ highWaterMark: 2 }).size,
);
assertStrictEquals(
  new ByteLengthQueuingStrategy({ highWaterMark: 1 }).size,
  new ByteLengthQueuingStrategy({ highWaterMark: 2 }).size,
);
assertEquals(new CountQueuingStrategy({ highWaterMark: 1 }).size.name, "size");
assertEquals(new ByteLengthQueuingStrategy({ highWaterMark: 1 }).size.name, "size");
assertEquals(new CountQueuingStrategy({ highWaterMark: 1 }).size(), 1);
assertEquals(new ByteLengthQueuingStrategy({ highWaterMark: 1 }).size(new Uint8Array(3)), 3);

// A disturbed-but-unlocked stream is refused as a request body.
const disturbed = new ReadableStream({
  start(controller) {
    controller.enqueue(new Uint8Array([1]));
  },
});
const peek = disturbed.getReader();
await peek.read();
peek.releaseLock();
assert(!disturbed.locked, "the disturbed stream is unlocked again");
assertThrows(
  () => new Request("http://example.test/", { method: "POST", body: disturbed, duplex: "half" }),
  TypeError,
  "ReadableStream is locked or disturbed",
);
