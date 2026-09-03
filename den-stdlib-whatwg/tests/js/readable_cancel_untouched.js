import { assertEquals } from "den:assert";

// ReadableStream.prototype.cancel is the native method, not a wrapper, and it
// already answers with a Promise.
const cancel = ReadableStream.prototype.cancel;
assertEquals("_inner" in cancel, false);
const outcome = new ReadableStream().cancel("why");
assertEquals(outcome instanceof Promise, true);
assertEquals(await outcome, undefined);
