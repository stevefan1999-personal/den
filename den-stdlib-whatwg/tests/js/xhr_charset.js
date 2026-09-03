import { assertEquals } from "den:assert";
import { serve } from "den:http";

// overrideMimeType reads a quoted charset the same way a Content-Type does.
const server = serve({
  listen: { host: "127.0.0.1", port: 0 },
  fetch: () =>
    new Response(Uint8Array.of(0xc9), {
      headers: { "access-control-allow-origin": "*", "content-type": "text/plain; charset=utf-8" },
    }),
});
const send = (mime) =>
  new Promise((resolve, reject) => {
    const xhr = new XMLHttpRequest();
    xhr.open("GET", `${server.url}latin1`);
    xhr.overrideMimeType(mime);
    xhr.onload = () => resolve(xhr.responseText);
    xhr.onerror = () => reject(new Error("xhr error " + xhr.status));
    xhr.send();
  });
try {
  assertEquals(await send("text/plain; charset=iso-8859-1"), "É");
  assertEquals(await send('text/plain; charset="iso-8859-1"'), "É");
  assertEquals(await send("text/plain; charset='iso-8859-1'"), "É");
  assertEquals(await send('text/plain; CHARSET="iso-8859-1"; x=y'), "É");
  assertEquals(await send("text/plain"), "\uFFFD");
} finally {
  await server.close();
}
