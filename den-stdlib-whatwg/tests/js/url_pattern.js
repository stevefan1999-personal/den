import { assertEquals } from "den:assert";
const pattern = new URLPattern({ pathname: "/books/:id" });
assertEquals(pattern.test("https://x/books/1"), true);
assertEquals(pattern.test("https://x/authors/1"), false);
assertEquals(pattern.exec("https://x/books/1").pathname.groups.id, "1");

// A full URL pattern string must decompose into components, not be jammed
// whole into `pathname`.
const full = new URLPattern("https://example.com/books/:id");
assertEquals(full.protocol, "https");
assertEquals(full.hostname, "example.com");
assertEquals(full.pathname, "/books/:id");
assertEquals(full.test("https://example.com/books/1"), true);
assertEquals(full.test("https://other.com/books/1"), false);
assertEquals(full.exec("https://example.com/books/1").pathname.groups.id, "1");

const relative = new URLPattern("/books/:id", "https://example.com");
assertEquals(relative.hostname, "example.com");
assertEquals(relative.test("/books/1", "https://example.com"), true);
assertEquals(relative.test("https://other.com/books/1"), false);
assertEquals(new URLPattern("https://*.example.com/*").hostname, "*.example.com");

// A relative pattern string with no base is a TypeError, as in Deno.
let threw = false;
try {
  new URLPattern("/books/:id");
} catch (error) {
  threw = error instanceof TypeError;
}
assertEquals(threw, true);

// An unparseable match target is "no match", not a throw.
assertEquals(full.test("not a url"), false);
assertEquals(full.exec("not a url"), null);

// exec() reports all eight components plus the original inputs.
const matched = new URLPattern("https://example.com/books/:id").exec(
  "https://example.com/books/1",
);
assertEquals(Object.keys(matched), [
  "inputs",
  "protocol",
  "username",
  "password",
  "hostname",
  "port",
  "pathname",
  "search",
  "hash",
]);
assertEquals(matched.inputs, ["https://example.com/books/1"]);
assertEquals(matched.hostname.input, "example.com");
assertEquals(
  new URLPattern("/books/:id", "https://example.com").exec(
    "/books/1",
    "https://example.com",
  ).inputs,
  ["/books/1", "https://example.com"],
);

// Every component getter, hasRegExpGroups, the brand and the options bag.
const every = new URLPattern("https://user:pw@example.com:8080/x?q=1#f");
assertEquals(
  [
    every.protocol,
    every.username,
    every.password,
    every.hostname,
    every.port,
    every.pathname,
    every.search,
    every.hash,
  ],
  ["https", "user:pw", "*", "example.com", "8080", "/x", "q=1", "f"],
);
assertEquals(new URLPattern({ pathname: "/x/:id" }).hasRegExpGroups, false);
assertEquals(new URLPattern({ pathname: "/x/:id(\\d+)" }).hasRegExpGroups, true);
assertEquals(Object.prototype.toString.call(every), "[object URLPattern]");
assertEquals(new URLPattern().pathname, "*");
assertEquals(
  new URLPattern({ pathname: "/FOO" }, { ignoreCase: true }).test(
    "https://x/foo",
  ),
  true,
);
assertEquals(
  new URLPattern("/FOO", "https://x", { ignoreCase: true }).test(
    "https://x/foo",
  ),
  true,
);
