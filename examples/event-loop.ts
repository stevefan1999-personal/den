// cargo run -- examples/event-loop.ts
//
// den's process event loop is AsyncRuntime::idle(): the binary stays up
// while any spawned future is alive (timers, socket accept, worker ports,
// top-level await). There is no extra "run loop" API. This file shows the
// TypeScript side of that — EventTarget, an async iterator of timer ticks,
// and a ReadableStream drained with for-await.

export {};

async function* ticks(ms: number, count: number): AsyncGenerator<number> {
  for (let i = 0; i < count; i++) {
    await new Promise<void>((resolve) => {
      setTimeout(resolve, ms);
    });
    yield i;
  }
}

async function* chunks(stream: ReadableStream<Uint8Array>): AsyncGenerator<Uint8Array> {
  const reader = stream.getReader();
  for (;;) {
    const { value, done } = await reader.read();
    if (done) return;
    yield value;
  }
}

const bus = new EventTarget();
const seen: number[] = [];
bus.addEventListener("tick", (event: Event) => {
  const custom = event as CustomEvent<number>;
  seen.push(custom.detail);
  console.log("tick event", custom.detail, "at", Temporal.Now.instant().toString());
});

for await (const n of ticks(15, 3)) {
  bus.dispatchEvent(new CustomEvent("tick", { detail: n }));
}

const blob = new Blob(["hello den"], { type: "text/plain" });
let text = "";
for await (const chunk of chunks(blob.stream())) {
  text += new TextDecoder().decode(chunk);
}
console.log("streamed blob", JSON.stringify(text));
console.log("ticks", seen.join(","));
