import { assertEquals } from "den:assert";

const infoOf = (fn) => {
  try {
    fn();
    return "none";
  } catch (error) {
    return [
      error instanceof DOMException ? "dom" : "plain",
      error.name,
      error.code,
    ].join(":");
  }
};
const ws = new WebSocket("ws://127.0.0.1:1/");
ws.onopen = () => {};
ws.onerror = () => {};
ws.addEventListener("close", () => {});
ws.binaryType = "nope";
const kept = ws.binaryType;
ws.binaryType = "arraybuffer";
assertEquals(
  [
    infoOf(() => new WebSocket("not a url")),
    infoOf(() => new WebSocket("http://example.com/")),
    infoOf(() => new WebSocket("ws://example.com/#frag")),
    infoOf(() => new WebSocket("ws://user@example.com/")),
    infoOf(() => new WebSocket("ws://example.com/", ["a", "a"])),
    infoOf(() => new WebSocket("ws://example.com/", ["bad protocol"])),
    infoOf(() => ws.send("early")),
    infoOf(() => ws.close(1001)),
    infoOf(() => ws.close(1000, "x".repeat(124))),
    kept,
    ws.binaryType,
    ws.CONNECTING === 0,
  ].join("|"),
  "dom:SyntaxError:12|dom:SyntaxError:12|dom:SyntaxError:12|dom:SyntaxError:12|dom:SyntaxError:12|dom:SyntaxError:12|dom:InvalidStateError:11|dom:InvalidAccessError:15|dom:SyntaxError:12|blob|arraybuffer|true",
);
