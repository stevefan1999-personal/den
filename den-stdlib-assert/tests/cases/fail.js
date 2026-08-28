import {
  AssertionError,
  assertIsError,
  assertThrows,
  fail,
  unimplemented,
  unreachable,
} from "den:assert";

assertThrows(() => fail("nope"), AssertionError, "nope");
assertThrows(() => unimplemented(), AssertionError, "unimplemented");
assertThrows(() => unreachable(), AssertionError, "unreachable");

const error = new TypeError("boom");
assertIsError(error, TypeError, "boom");
assertThrows(() => assertIsError("not an error"), AssertionError, "Expected an Error");
