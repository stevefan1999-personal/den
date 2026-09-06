// cargo run -- examples/unix-echo.ts
//
// Same shape as tcp-echo.ts, over a pathname Unix socket. The listener's
// local address is the path string, not a SocketAddr.

import { assertEquals } from "den:assert";
import { UnixListener, UnixStream } from "den:networking";
import { connections, dest, writeAll } from "./net.ts";
import { uniquePath } from "./temp.ts";

const path = uniquePath("den-example-unix", ".sock");
const listener = await UnixListener.listen(path);
console.log("listening", dest(listener));

const server = (async (): Promise<void> => {
  for await (const { stream, peer } of connections(listener)) {
    const chunk: Uint8Array = await stream.read(64);
    const text = new TextDecoder().decode(chunk);
    assertEquals(text, "ping");
    console.log("server received", JSON.stringify(text), "from", peer.toString());
    await writeAll(stream, "pong");
    await stream.shutdown();
    break;
  }
})();

const client = (async (): Promise<void> => {
  const stream = await UnixStream.connect(path);
  await writeAll(stream, "ping");
  const reply = new TextDecoder().decode(await stream.read(64));
  assertEquals(reply, "pong");
  console.log("client received", JSON.stringify(reply));
  await stream.shutdown();
})();

await Promise.all([server, client]);
