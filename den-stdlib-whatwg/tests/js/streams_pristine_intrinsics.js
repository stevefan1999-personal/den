// Reactions that must keep a JS value alive ride it as a bound leading
// argument through the realm's pristine Function.prototype.bind: a script that
// patches `bind` must neither observe nor break the stream machinery.
import { assertEquals, assertStrictEquals } from "den:assert";

const observed = [];
const pristineBind = Function.prototype.bind;
Function.prototype.bind = function (...args) {
  observed.push("bind");
  return pristineBind.apply(this, args);
};

const outcome = async (promise) => {
  try {
    return ["fulfilled", await promise];
  } catch (error) {
    return ["rejected", error];
  }
};
const failure = new Error("boom");

// start, pull and their rejections on the readable side.
const started = new ReadableStream({
  start() {
    return Promise.resolve();
  },
  pull(controller) {
    controller.enqueue(1);
    controller.close();
  },
});
const startedReader = started.getReader();
assertEquals((await startedReader.read()).value, 1);
assertEquals((await startedReader.read()).done, true);
const startFails = new ReadableStream({
  start() {
    return Promise.reject(failure);
  },
});
assertStrictEquals((await outcome(startFails.getReader().read()))[1], failure);
const pullFails = new ReadableStream({
  pull() {
    return Promise.reject(failure);
  },
});
assertStrictEquals((await outcome(pullFails.getReader().read()))[1], failure);

// start, write, close and abort and their rejections on an async sink.
const written = [];
const sinkWriter = new WritableStream({
  write(chunk) {
    written.push(chunk);
    return Promise.resolve();
  },
  close() {
    return Promise.resolve();
  },
}).getWriter();
await sinkWriter.write("a");
await sinkWriter.close();
assertEquals(written, ["a"]);
const writeFails = new WritableStream({
  write() {
    return Promise.reject(failure);
  },
}).getWriter();
assertStrictEquals((await outcome(writeFails.write("x")))[1], failure);
const closeFails = new WritableStream({
  close() {
    return Promise.reject(failure);
  },
}).getWriter();
assertStrictEquals((await outcome(closeFails.close()))[1], failure);
const sinkStartFails = new WritableStream({
  start() {
    return Promise.reject(failure);
  },
}).getWriter();
assertStrictEquals((await outcome(sinkStartFails.closed))[1], failure);
const abortable = new WritableStream({
  abort() {
    return Promise.resolve();
  },
}).getWriter();
assertEquals(await outcome(abortable.abort(failure)), ["fulfilled", undefined]);

// A pipe with an already-aborted signal, a pipe whose destination must be
// aborted, a pipeThrough left early by the async iterator, and an explicit
// iterator return.
const events = [];
const recorder = () =>
  new WritableStream({
    write(chunk) {
      events.push(chunk);
    },
    abort(reason) {
      events.push(reason);
    },
  });
const source = (chunks) =>
  new ReadableStream({
    start(controller) {
      for (const chunk of chunks) controller.enqueue(chunk);
      controller.close();
    },
  });
assertStrictEquals(
  (await outcome(source([1]).pipeTo(recorder(), { signal: AbortSignal.abort(failure) })))[1],
  failure,
);
const erroredSource = new ReadableStream({
  start(controller) {
    controller.error(failure);
  },
});
assertStrictEquals((await outcome(erroredSource.pipeTo(recorder())))[1], failure);
// Both pipes abort their destination with the failure.
assertEquals(events, [failure, failure]);
const doubled = source([1, 2, 3]).pipeThrough(
  new TransformStream({
    transform(chunk, controller) {
      controller.enqueue(chunk * 2);
    },
  }),
);
const seen = [];
for await (const chunk of doubled) {
  seen.push(chunk);
  if (seen.length === 2) break;
}
assertEquals(seen, [2, 4]);
const iterator = source([7, 8])[Symbol.asyncIterator]();
assertEquals((await iterator.next()).value, 7);
assertEquals(await iterator.return("early"), { value: "early", done: true });

// A transformer whose start rejects: the one reaction that used to hold its
// controller for the rest of the run.
const transformStartFails = new TransformStream({
  start() {
    return Promise.reject(failure);
  },
});
assertStrictEquals((await outcome(transformStartFails.readable.getReader().read()))[1], failure);

assertEquals(observed, []);

// Teardown probe: a pipeThrough nobody drains, left in flight when the realm
// is dropped. Reaching the end of the file cleanly is the assertion.
globalThis.stalled = source(["never read"]).pipeThrough(new TransformStream());
