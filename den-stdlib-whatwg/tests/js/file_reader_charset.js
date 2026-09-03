import { assertEquals } from "den:assert";

// The type's charset parameter may be bare, double- or single-quoted, in any
// case, and followed by more parameters.
const read = (type) =>
  new Promise((resolve, reject) => {
    const reader = new FileReader();
    reader.onload = () => resolve(reader.result);
    reader.onerror = () => reject(reader.error);
    reader.readAsText(new Blob([Uint8Array.of(0xc9)], { type }));
  });
assertEquals(await read("text/plain; charset=iso-8859-1"), "É");
assertEquals(await read('text/plain; charset="iso-8859-1"'), "É");
assertEquals(await read("text/plain; charset='iso-8859-1'"), "É");
assertEquals(await read('text/plain; CHARSET="ISO-8859-1"; x=y'), "É");
assertEquals(await read("text/plain"), "\uFFFD");
