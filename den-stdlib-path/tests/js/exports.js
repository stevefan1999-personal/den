import { assert, assertEquals } from "den:assert";
import path, { delimiter, posix, sep, windows } from "den:path";

assertEquals(posix.sep, "/");
assertEquals(posix.delimiter, ":");
assertEquals(windows.sep, "\\");
assertEquals(windows.delimiter, ";");
assert(sep === posix.sep || sep === windows.sep);
assert(delimiter === posix.delimiter || delimiter === windows.delimiter);
assertEquals(typeof path.join, "function");
assertEquals(typeof posix.posix.join, "function");
assertEquals(typeof posix.windows.normalize, "function");
assertEquals(typeof windows.posix.basename, "function");
assertEquals(path.posix.sep, "/");
assertEquals(path.windows.sep, "\\");
