import { assertEquals, assertThrows } from "den:assert";

// Each Headers guard keeps its rule after a Request or Response is built
// around it and after the headers are cloned into a fresh Headers.
const immutable = Response.error().headers;
assertThrows(() => immutable.set("x-a", "1"), TypeError, "Headers are immutable");
assertThrows(() => immutable.append("x-a", "1"), TypeError, "Headers are immutable");
assertThrows(() => immutable.delete("x-a"), TypeError, "Headers are immutable");
assertEquals(new Headers(immutable).has("x-a"), false);

const request = new Request("http://127.0.0.1/", { headers: { host: "evil", "x-a": "1" } });
request.headers.set("cookie", "a=b");
assertEquals([...request.headers.keys()], ["x-a"]);
const noCors = new Request("http://127.0.0.1/", { mode: "no-cors", headers: { "x-a": "1" } });
noCors.headers.set("accept", "text/plain");
noCors.headers.set("content-type", "application/json");
assertEquals([...noCors.headers.keys()], ["accept"]);

const response = new Response(null, { headers: { "x-a": "1" } });
response.headers.set("set-cookie", "a=b");
assertEquals([...response.headers.keys()], ["x-a"]);
const copy = new Headers(response.headers);
copy.set("set-cookie", "a=b");
assertEquals([...copy.keys()], ["set-cookie", "x-a"]);
