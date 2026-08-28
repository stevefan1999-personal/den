import { assertEquals } from "den:assert";
import { windows } from "den:path";

assertEquals(windows.normalize("a//b//../b"), "a\\b");
assertEquals(windows.normalize("//server/share/dir/../../../file"), "\\\\server\\share\\file");
assertEquals(windows.normalize("/"), "\\");
assertEquals(windows.dirname("C:\\foo\\bar"), "C:\\foo");
assertEquals(windows.basename("C:\\foo\\bar.txt", ".txt"), "bar");
assertEquals(windows.extname("C:\\foo\\archive.tar.gz"), ".gz");
assertEquals(windows.isAbsolute("C:\\foo"), true);
assertEquals(windows.isAbsolute("C:foo"), false);
assertEquals(windows.isAbsolute("\\\\server\\share"), true);
assertEquals(windows.isAbsolute("bar"), false);
assertEquals(windows.join("C:\\foo", "bar\\..\\baz"), "C:\\foo\\baz");
assertEquals(windows.relative("C:\\a\\b", "C:\\a\\c"), "..\\c");
assertEquals(windows.relative("C:\\a", "D:\\b"), "D:\\b");
assertEquals(windows.toNamespacedPath("C:\\foo"), "\\\\?\\C:\\foo");
assertEquals(windows.toNamespacedPath("\\\\server\\share\\file"), "\\\\?\\UNC\\server\\share\\file");

const parsed = windows.parse("C:\\home\\user\\file.txt");
assertEquals(parsed.root, "C:\\");
assertEquals(parsed.base, "file.txt");
assertEquals(parsed.ext, ".txt");
assertEquals(parsed.name, "file");
assertEquals(windows.format(parsed), "C:\\home\\user\\file.txt");
