import { assert, assertEquals } from "den:assert";

const url = process.env.DEN_TEST_WS_URL;
const ws = new WebSocket(url);
await new Promise((resolve, reject) => {
  ws.onopen = () => resolve();
  ws.onerror = (event) => reject(new Error(event.message || "ws error"));
});
ws.send("ping");
const data = await new Promise((resolve) => {
  ws.onmessage = (event) => resolve(event.data);
});
ws.close();
assertEquals(data, "ping");
assert(ws.readyState === WebSocket.CLOSING || ws.readyState === WebSocket.CLOSED);
assert(ws instanceof EventTarget);
assertEquals(WebSocket.CONNECTING, 0);
assertEquals(WebSocket.OPEN, 1);
