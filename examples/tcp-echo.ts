// cargo run -- examples/tcp-echo.ts
//
// One process is both sides: TcpListener.accept and TcpStream.connect
// overlap on the event loop. The accept side is an async iterator so a
// real server can `for await` connections instead of writing a recursive
// accept() loop.

import { TcpListener, TcpStream } from "den:networking";
import { connections, dest, writeAll } from "./net.ts";

const listener = await TcpListener.listen("127.0.0.1:0");
const address: string = dest(listener);
console.log("listening", address);

const server = (async (): Promise<void> => {
  for await (const { stream, peer } of connections(listener)) {
    const chunk: Uint8Array = await stream.read(64);
    const text: string = new TextDecoder().decode(chunk);
    console.log("server received", JSON.stringify(text), "from", peer.toString());
    await writeAll(stream, "pong");
    await stream.shutdown();
    break;
  }
})();

const client = (async (): Promise<void> => {
  const stream = await TcpStream.connect(address);
  await writeAll(stream, "ping");
  const reply: string = new TextDecoder().decode(await stream.read(64));
  console.log("client received", JSON.stringify(reply));
  await stream.shutdown();
})();

await Promise.all([server, client]);
