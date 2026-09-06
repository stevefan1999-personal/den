// cargo run -- examples/canvas.ts
//
// Canvas phase 0: ImageData and createImageBitmap over those two sources.
// There is no 2D context and no codec, so Blob sources wait. Pixel read-back
// is den's Symbol.for("den.bitmapData") hook — ImageBitmap has no spec-visible
// pixels until something can draw it.

import { assertEquals } from "den:assert";

const source = new ImageData(2, 1);
source.data.set([255, 0, 0, 255, 0, 255, 0, 255]);

const bitmap = await createImageBitmap(source);
const pixels = Array.from(
  (bitmap as unknown as Record<symbol, () => Uint8Array>)[Symbol.for("den.bitmapData")](),
);
assertEquals(bitmap.width, 2);
assertEquals(bitmap.height, 1);
assertEquals(pixels, [255, 0, 0, 255, 0, 255, 0, 255]);
console.log("bitmap", bitmap.width, "x", bitmap.height, pixels);

bitmap.close();
assertEquals(bitmap.width, 0);
assertEquals(bitmap.height, 0);
console.log("closed", bitmap.width, bitmap.height);
