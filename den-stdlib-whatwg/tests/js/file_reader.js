import { assert, assertEquals } from "den:assert";
const reader = new FileReader();
const result = await new Promise((resolve, reject) => {
  reader.onload = () => resolve(reader.result);
  reader.onerror = () => reject(reader.error);
  reader.readAsText(new Blob(["hello"]));
});
assertEquals(result, "hello");
assertEquals(reader.readyState, FileReader.DONE);
assert(reader instanceof EventTarget);
