import { assert, assertEquals } from "den:assert";
const reader = new FileReader();
let trusted;
const result = await new Promise((resolve, reject) => {
  reader.onload = (event) => {
    trusted = event.isTrusted;
    resolve(reader.result);
  };
  reader.onerror = () => reject(reader.error);
  reader.readAsText(new Blob(["hello"]));
});
assertEquals(result, "hello");
assertEquals(reader.readyState, FileReader.DONE);
assert(trusted);
assert(reader instanceof EventTarget);
