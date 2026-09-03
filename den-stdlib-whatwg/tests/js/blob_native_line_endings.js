import { assertEquals } from "den:assert";

// `endings: "native"` folds CRLF and a lone CR to LF, part by part; LF and
// every other byte pass through untouched.
const native = new Blob(["a\r\nb\rc\nd\r", "\ne"], { endings: "native" });
assertEquals(await native.text(), "a\nb\nc\nd\n\ne");
const transparent = new Blob(["a\r\nb\rc\n"], { endings: "transparent" });
assertEquals(await transparent.text(), "a\r\nb\rc\n");
