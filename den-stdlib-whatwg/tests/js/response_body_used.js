import { assertEquals } from "den:assert";

const empty = new Response();
assertEquals(empty.bodyUsed, false);

const blob = new Response(new Blob(["x"]));
assertEquals(blob.bodyUsed, false);
assertEquals(await blob.text(), "x");
assertEquals(blob.bodyUsed, true);
