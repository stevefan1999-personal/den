import { assertEquals } from "den:assert";
const get = await fetch(process.env.DEN_TEST_GET_URL);
const json = await get.json();
const posted = await fetch(process.env.DEN_TEST_POST_URL, {
  method: "POST",
  headers: { "Content-Type": "text/plain" },
  body: "ping",
});
const echoed = await posted.text();
assertEquals(get.status, 200);
assertEquals(json.ok, true);
assertEquals(posted.status, 200);
assertEquals(echoed, "ping");
assertEquals(get.type, "cors");
assertEquals(posted.headers.get("content-type"), "text/plain");
