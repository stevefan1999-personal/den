import * as ns from "den:assert";
import { assertEquals } from "den:assert";

assertEquals(
  Object.keys(ns).sort().join(","),
  "AssertionError,assert,assertAlmostEquals,assertArrayIncludes,assertEquals,assertExists,assertFalse,assertGreater,assertGreaterOrEqual,assertInstanceOf,assertIsError,assertLess,assertLessOrEqual,assertMatch,assertNotEquals,assertNotInstanceOf,assertNotMatch,assertNotStrictEquals,assertObjectMatch,assertRejects,assertStrictEquals,assertStringIncludes,assertThrows,equal,fail,unimplemented,unreachable",
);
