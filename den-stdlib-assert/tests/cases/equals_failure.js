import { assertEquals } from "den:assert";
try {
  assertEquals(1, 2);
  throw new Error("expected a throw");
} catch (error) {
  assertEquals(error.message, "Values are not equal: 1 !== 2");
}
