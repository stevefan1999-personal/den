const { assert, assertEquals } = await import("den:assert");
const blob = new Blob(["hello ", "world"], { type: "text/plain" });
const file = new File(["x"], "x.txt", { type: "text/plain", lastModified: 1 });
const form = new FormData();
form.append("a", "1");
const reader = new FileReader();
const text = await blob.text();
const read = await new Promise((resolve, reject) => {
  reader.onload = () => resolve(reader.result);
  reader.onerror = () => reject(reader.error);
  reader.readAsText(new Blob(["from-reader"]));
});
assertEquals(blob.size, 11);
assertEquals(text, "hello world");
assert(file instanceof Blob);
assertEquals(file.name, "x.txt");
assertEquals(form.get("a"), "1");
assert(reader instanceof EventTarget);
assertEquals(read, "from-reader");
for (const ctor of [
  Blob, File, FileReader, FormData, XMLHttpRequest, EventSource, URLPattern,
  CompressionStream, DecompressionStream, WebSocket,
]) {
  assertEquals(typeof ctor, "function");
}
