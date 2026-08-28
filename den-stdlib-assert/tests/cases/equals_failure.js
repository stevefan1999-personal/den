import { assertEquals, assertSnapshot } from "den:assert";
try {
  assertEquals(1, 2);
  throw new Error("expected a throw");
} catch (error) {
  assertSnapshot(error.message, "den_assert_equals_failure");
}
