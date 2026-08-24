import { assertEquals } from "den:assert";

await (async () => {
  const stream = new ReadableStream({
    start(controller) {
      controller.enqueue(new Uint8Array([1, 2]));
      controller.enqueue(new Uint8Array([3]));
      controller.close();
    },
  });
  const reader = stream.getReader();
  const out = [];
  for (;;) {
    const { done, value } = await reader.read();
    if (done) break;
    out.push(...value);
  }
  assertEquals(out.join(","), "1,2,3");
})();
