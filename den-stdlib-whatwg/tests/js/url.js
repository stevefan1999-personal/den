import { assert, assertEquals } from "den:assert";

const url = new URL(
  "../c?b=2&a=1&a=3#frag",
  "https://User:Pass@EXAMPLE.com:443/a/b/",
);
const params = url.searchParams;
assertEquals(url.searchParams, params);

params.append("space", "a b");
params.set("a", "9");
params.sort();
const afterParams = {
  href: url.href,
  origin: url.origin,
  protocol: url.protocol,
  username: url.username,
  password: url.password,
  host: url.host,
  hostname: url.hostname,
  port: url.port,
  pathname: url.pathname,
  search: url.search,
  hash: url.hash,
  entries: [...params],
};

url.search = "?x=1&x=2";
assertEquals(url.searchParams, params);
assertEquals(params.getAll("x").join(","), "1,2");
params.delete("x", "1");
params.append("z", "✓");
assertEquals(url.search, "?x=2&z=%E2%9C%93");

const standalone = new URLSearchParams({ b: 2, a: "x y" });
standalone.append("a", "last");
assert(URL.canParse("/ok", "https://example.com"));
assertEquals(URL.parse("not a url"), null);

globalThis.snapshot = JSON.stringify({
  afterParams,
  afterSearch: {
    href: url.href,
    entries: [...params],
    size: params.size,
  },
  standalone: {
    text: standalone.toString(),
    keys: [...standalone.keys()],
    values: [...standalone.values()],
  },
}, null, 2);
