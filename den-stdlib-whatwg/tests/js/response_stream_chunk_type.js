import { assertEquals } from "den:assert";

// A body stream may only carry BufferSource chunks; anything else fails the
// consumer with one fixed TypeError.
for (const chunk of ["text", 1, true, null, undefined, {}]) {
  const response = new Response(
    new ReadableStream({ start(controller) { controller.enqueue(chunk); } }),
  );
  let caught;
  try {
    await response.text();
  } catch (error) {
    caught = error;
  }
  assertEquals(caught instanceof TypeError, true);
  assertEquals(caught.message, "ReadableStream chunk must be a Uint8Array");
}
