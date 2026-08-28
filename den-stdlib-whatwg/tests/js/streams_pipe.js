// Regressions for the three composition primitives that used to be silent
// no-ops: piping wrote once and never closed, piping through a transformer
// moved nothing at all, and tee handed back a locked original.
import { assert, assertEquals } from "den:assert";

const events = [];
const recorder = () =>
  new WritableStream({
    write(chunk) {
      events.push(["write", Array.from(chunk).join(",")]);
    },
    close() {
      events.push(["close"]);
    },
    abort(reason) {
      events.push(["abort", String(reason && reason.message)]);
    },
  });

const source = (chunks) =>
  new ReadableStream({
    start(controller) {
      for (const chunk of chunks) controller.enqueue(chunk);
      controller.close();
    },
  });

// Every chunk arrives separately, in order, followed by exactly one close.
await source([new Uint8Array([1, 2]), new Uint8Array([3, 4, 5])]).pipeTo(recorder());
assertEquals(JSON.stringify(events), JSON.stringify([["write", "1,2"], ["write", "3,4,5"], ["close"]]));

// preventClose leaves the destination open.
events.length = 0;
const open = recorder();
await source([new Uint8Array([9])]).pipeTo(open, { preventClose: true });
assertEquals(JSON.stringify(events), JSON.stringify([["write", "9"]]));
assertEquals(open.locked, false);

// A destination that is not a WritableStream rejects instead of resolving.
let rejected = false;
await source([]).pipeTo({}).catch(() => (rejected = true));
assert(rejected, "pipeTo({}) must reject");

// An already-aborted signal rejects with the signal's reason and never writes.
events.length = 0;
let aborted;
await source([new Uint8Array([1])])
  .pipeTo(recorder(), { signal: AbortSignal.abort(new Error("nope")) })
  .catch((error) => (aborted = error));
assertEquals(String(aborted && aborted.message), "nope");
assertEquals(events.filter(([kind]) => kind === "write").length, 0);

// pipeThrough actually pumps the transformer.
const doubled = source([1, 2, 3]).pipeThrough(
  new TransformStream({
    transform(chunk, controller) {
      controller.enqueue(chunk * 2);
    },
    flush(controller) {
      controller.enqueue("end");
    },
  }),
);
const out = [];
for await (const chunk of doubled) out.push(chunk);
assertEquals(JSON.stringify(out), JSON.stringify([2, 4, 6, "end"]));

// tee gives two independent branches, and neither is the locked original.
const [left, right] = source(["a", "b"]).tee();
const drain = async (stream) => {
  const chunks = [];
  for await (const chunk of stream) chunks.push(chunk);
  return chunks.join("");
};
assertEquals(await drain(left), "ab");
assertEquals(await drain(right), "ab");

// ReadableStream.from accepts a sync iterable and an async one.
assertEquals(await drain(ReadableStream.from(["x", "y"])), "xy");
assertEquals(
  await drain(
    ReadableStream.from(
      (async function* () {
        yield "p";
        yield "q";
      })(),
    ),
  ),
  "pq",
);

// desiredSize is the strategy's high-water mark minus what is queued.
const paced = new ReadableStream({
  start(controller) {
    assertEquals(controller.desiredSize, 3);
    controller.enqueue("one");
    assertEquals(controller.desiredSize, 2);
  },
}, { highWaterMark: 3 });
assert(paced.locked === false);

// writer.ready is a real capacity signal, and a released writer is dead.
const sink = new WritableStream({ write() {} }, new CountQueuingStrategy({ highWaterMark: 1 }));
const writer = sink.getWriter();
assertEquals(writer.desiredSize, 1);
await writer.ready;
writer.releaseLock();
let released = false;
await writer.write("x").catch(() => (released = true));
assert(released, "a released writer must reject");

// The deferred pieces throw instead of silently degrading.
for (const build of [
  () => new ReadableStream({ type: "bytes" }),
  () => new ReadableStream({}).getReader({ mode: "byob" }),
]) {
  let threw = false;
  try {
    build();
  } catch (error) {
    threw = error instanceof TypeError;
  }
  assert(threw, "an unimplemented stream feature must throw a TypeError");
}

// Teardown probes. Every shape below leaves an operation in flight when the
// realm is dropped, which is where QuickJS asserts that no JS object is still
// referenced from Rust. A stream graph that Rust pins for an operation that
// never completes used to abort the process here, so these assert nothing:
// reaching the end of the file is the assertion.
const stalled = new TransformStream();
globalThis.stalledReadable = new ReadableStream({
  start(controller) {
    controller.enqueue("never read");
    controller.close();
  },
}).pipeThrough(stalled);

const [leftBranch, rightBranch] = new ReadableStream({
  start(controller) {
    controller.enqueue("never read either");
    controller.close();
  },
}).tee();
globalThis.stalledBranches = [leftBranch, rightBranch];

// A writer parked on a full queue, and a reader parked on an empty stream.
const parked = new WritableStream({ write: () => new Promise(() => {}) });
const parkedWriter = parked.getWriter();
globalThis.parkedWrite = parkedWriter.write("first");
globalThis.parkedNext = parkedWriter.write("second");
globalThis.parkedRead = new ReadableStream({ start() {} }).getReader().read();

// A pipe watching a live AbortSignal. The signal's listener list and the
// listener must not hold each other over an edge the collector cannot see.
globalThis.watchedPipe = new ReadableStream().pipeThrough(
  { writable: new WritableStream(), readable: new ReadableStream() },
  { signal: new AbortController().signal },
);
