import { assertEquals } from "den:assert";

const source = new EventTarget();
const given = new MessageEvent("message", {
  data: 1, origin: "https://den.example", lastEventId: "7", source,
});
const bare = new MessageEvent("message");
assertEquals(given.origin, "https://den.example");
assertEquals(given.lastEventId, "7");
assertEquals(given.source === source, true);
assertEquals(`${bare.origin}|${bare.lastEventId}|${bare.source}`, "||null");
assertEquals(new MessageEvent("message", { origin: 7 }).origin, "7");
