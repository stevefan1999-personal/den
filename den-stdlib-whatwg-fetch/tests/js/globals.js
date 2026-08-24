import { assertEquals } from "den:assert";
for (const name of ["Headers", "Request", "fetch", "Response"]) {
  assertEquals(typeof globalThis[name], "function");
}
