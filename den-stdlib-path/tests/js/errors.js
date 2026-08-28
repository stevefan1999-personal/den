import { assertThrows } from "den:assert";
import { posix } from "den:path";

assertThrows(() => posix.normalize(), TypeError, "path must be a string");
assertThrows(() => posix.normalize(1), TypeError, "path must be a string");
assertThrows(() => posix.join("/tmp", 2), TypeError, "path must be a string");
assertThrows(() => posix.relative("/a"), TypeError, "to must be a string");
assertThrows(() => posix.basename("/foo", 1), TypeError, "suffix must be a string");
assertThrows(() => posix.matchesGlob("a.js"), TypeError, "pattern must be a string");
assertThrows(() => posix.format(null), TypeError, "pathObject must be an object");
assertThrows(() => posix.format([]), TypeError, "pathObject must be an object");
