import { assert, assertEquals } from "den:assert";

const href = "https://cdn.jsdelivr.net/npm/ms@2.1.3/package.json";
const url = new URL(href);
const pattern = new URLPattern({ pathname: "/npm/:pkg/:file" });
assert(pattern.test(url));
assertEquals(pattern.exec(url).pathname.groups.pkg, "ms@2.1.3");
assertEquals(pattern.exec(url).pathname.groups.file, "package.json");

const response = await fetch(url);
const pkg = await response.json();
assertEquals(pkg.name, "ms");
