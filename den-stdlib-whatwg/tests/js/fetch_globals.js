import { assertEquals } from "den:assert";
import { fetch as importedFetch, Headers, Request, Response } from "den:whatwg-fetch";

for (const [name, imported] of Object.entries({ Headers, Request, fetch: importedFetch, Response })) {
  assertEquals(typeof imported, "function");
  assertEquals(imported, globalThis[name]);
}
