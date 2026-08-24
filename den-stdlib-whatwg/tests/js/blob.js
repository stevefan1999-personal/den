import { assert, assertEquals } from "den:assert";
const blob = new Blob(["hello ", "world"], { type: "text/plain" });
assertEquals(blob.size, 11);
assertEquals(blob.type, "text/plain");
assertEquals(await blob.text(), "hello world");
assertEquals(await blob.slice(6).text(), "world");
assertEquals((await blob.arrayBuffer()).byteLength, 11);
assert(blob instanceof Blob);
