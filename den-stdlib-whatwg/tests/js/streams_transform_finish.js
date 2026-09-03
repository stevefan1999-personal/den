// A transformer's flush or cancel settles the finish promise with its own
// outcome and errors the opposite half, on the close, cancel and abort paths.
import { assertEquals, assertStrictEquals } from "den:assert";

const outcome = async (promise) => {
  try {
    return ["fulfilled", await promise];
  } catch (error) {
    return ["rejected", error];
  }
};

// flush rejects: the writer's close rejects with it and the readable errors with it.
const flushFailure = new Error("flush failed");
const flushing = new TransformStream({
  flush() {
    return Promise.reject(flushFailure);
  },
});
const flushWriter = flushing.writable.getWriter();
const flushReader = flushing.readable.getReader();
const [closeState, closeReason] = await outcome(flushWriter.close());
assertEquals(closeState, "rejected");
assertStrictEquals(closeReason, flushFailure);
const [readState, readReason] = await outcome(flushReader.read());
assertEquals(readState, "rejected");
assertStrictEquals(readReason, flushFailure);

// flush fulfils: close resolves once the readable has drained what flush enqueued.
const clean = new TransformStream({
  flush(controller) {
    controller.enqueue("tail");
  },
});
const cleanWriter = clean.writable.getWriter();
const cleanReader = clean.readable.getReader();
const closing = cleanWriter.close();
assertEquals((await cleanReader.read()).value, "tail");
assertEquals((await cleanReader.read()).done, true);
assertEquals(await outcome(closing), ["fulfilled", undefined]);

// cancel rejects from the readable side: readable.cancel rejects with it and
// the writable errors with it.
const cancelFailure = new Error("cancel failed");
const cancelling = new TransformStream({
  cancel() {
    return Promise.reject(cancelFailure);
  },
});
const cancelWriter = cancelling.writable.getWriter();
const [cancelState, cancelReason] = await outcome(cancelling.readable.cancel(new Error("reader left")));
assertEquals(cancelState, "rejected");
assertStrictEquals(cancelReason, cancelFailure);
const [writeState, writeReason] = await outcome(cancelWriter.write("late"));
assertEquals(writeState, "rejected");
assertStrictEquals(writeReason, cancelFailure);

// cancel rejects from the writable side: abort rejects with it and the
// readable errors with it.
const aborting = new TransformStream({
  cancel() {
    return Promise.reject(cancelFailure);
  },
});
const abortReader = aborting.readable.getReader();
const [abortState, abortReason] = await outcome(aborting.writable.abort(new Error("writer left")));
assertEquals(abortState, "rejected");
assertStrictEquals(abortReason, cancelFailure);
const [abortReadState, abortReadReason] = await outcome(abortReader.read());
assertEquals(abortReadState, "rejected");
assertStrictEquals(abortReadReason, cancelFailure);
