import { assertEquals } from "den:assert";
const pattern = new URLPattern({ pathname: "/books/:id" });
assertEquals(pattern.test("https://x/books/1"), true);
assertEquals(pattern.test("https://x/authors/1"), false);
assertEquals(pattern.exec("https://x/books/1").pathname.groups.id, "1");
