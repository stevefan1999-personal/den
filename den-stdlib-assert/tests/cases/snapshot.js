import { assertSnapshot, assertThrows, AssertionError } from "den:assert";

assertSnapshot(
  JSON.stringify({ answer: 42, nested: [true, null] }),
  "den_assert_value_rendering",
);

assertThrows(
  () => assertSnapshot("x", "../invalid"),
  AssertionError,
  "snapshot name",
);
