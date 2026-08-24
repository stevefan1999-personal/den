import { assertEquals } from "den:assert";
for (const ctor of [
  Blob, CloseEvent, CompressionStream, DecompressionStream, EventSource, File,
  FileReader, FormData, ProgressEvent, ReadableStream, TransformStream,
  URLPattern, WebSocket, XMLHttpRequest,
]) {
  assertEquals(typeof ctor, "function");
  assertEquals(ctor.name, ctor.name);
}
