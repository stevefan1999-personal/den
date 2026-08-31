import { assertEquals } from "den:assert";
const signal = { aborted: true, addEventListener() {} };
let name = "not-aborted";
try {
  await fetch("http://127.0.0.1:1/", { signal });
} catch (error) {
  name = error.name;
}
assertEquals(name, "AbortError");
