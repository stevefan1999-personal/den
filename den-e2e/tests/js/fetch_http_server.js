import { assertEquals } from "den:assert";
import { TcpListener } from "den:networking";

const listener = await TcpListener.listen("127.0.0.1:0");
const addr = listener.localAddr ?? listener.local_addr;
const writeAll = (stream, bytes) => (stream.writeAll ?? stream.write_all).call(stream, bytes);
const body = "hello from den";
const cors =
  "Access-Control-Allow-Origin: *\r\nAccess-Control-Allow-Methods: *\r\nAccess-Control-Allow-Headers: *\r\nAccess-Control-Expose-Headers: *\r\n";

const serveOnce = async () => {
  const accepted = await listener.accept();
  const stream = accepted[0];
  const request = new TextDecoder().decode(await stream.read(8192));
  if (request.startsWith("OPTIONS")) {
    await writeAll(
      stream,
      `HTTP/1.1 204 No Content\r\n${cors}Connection: close\r\n\r\n`,
    );
    await stream.shutdown();
    return serveOnce();
  }
  await writeAll(
    stream,
    `HTTP/1.1 200 OK\r\nContent-Type: text/plain\r\nContent-Length: ${body.length}\r\n${cors}Connection: close\r\n\r\n${body}`,
  );
  await stream.shutdown();
};
const serving = serveOnce();

const url = new URL(`http://127.0.0.1:${addr.port}/`);
const pattern = new URLPattern({ pathname: "/" });
assertEquals(pattern.test(url), true);

const response = await fetch(url);
assertEquals(response.status, 200);
assertEquals(await response.text(), body);
await serving;
