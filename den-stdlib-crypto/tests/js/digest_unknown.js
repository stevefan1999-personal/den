import { assertEquals } from "den:assert";
const abc = new TextEncoder().encode("abc");
let name = "no rejection";
try {
  await crypto.subtle.digest("SHA-0", abc);
} catch (error) {
  name = error instanceof DOMException ? error.name : `wrong: ${error}`;
}
assertEquals(name, "NotSupportedError");
