import { assertEquals } from "den:assert";

// The body is produced lazily: `pull` is only asked for the next chunk once
// the transport has taken the previous one, so a request body that is still
// being generated is already on the wire.
const encoder = new TextEncoder();
const parts = ["strea", "med-", "upload"];
let pulled = 0;
const body = new ReadableStream({
  pull(controller) {
    if (pulled === parts.length) {
      controller.close();
      return;
    }
    controller.enqueue(encoder.encode(parts[pulled++]));
  },
});

const echoed = await fetch(process.env.DEN_TEST_UPLOAD_URL, {
  method: "POST",
  body,
  duplex: "half",
});
assertEquals(await echoed.text(), "streamed-upload");
assertEquals(pulled, parts.length);

// Without `duplex` the request is not the one the caller asked for.
let refused = null;
try {
  new Request(process.env.DEN_TEST_UPLOAD_URL, {
    method: "POST",
    body: new ReadableStream(),
  });
} catch (error) {
  refused = error;
}
assertEquals(refused instanceof TypeError, true);
