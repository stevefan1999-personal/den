import * as ns from "den:assert";
import { assertSnapshot } from "den:assert";

assertSnapshot(
  Object.keys(ns).sort().join("\n"),
  "den_assert_exports",
);
