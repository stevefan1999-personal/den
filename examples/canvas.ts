// cargo run -- examples/canvas.ts
//
// Canvas phase 0: ImageData and createImageBitmap over those two sources.
// There is no 2D context and no codec, so Blob sources wait. Pixel read-back
// is den's Symbol.for("den.bitmapData") hook — ImageBitmap has no spec-visible
// pixels until something can draw it.

export {};

const source = new ImageData(2, 1);
source.data.set([255, 0, 0, 255, 0, 255, 0, 255]);

const bitmap = await createImageBitmap(source);
const pixels = (bitmap as unknown as Record<symbol, () => Uint8Array>)[
  Symbol.for("den.bitmapData")
]();
console.log("bitmap", bitmap.width, "x", bitmap.height, Array.from(pixels));

bitmap.close();
console.log("closed", bitmap.width, bitmap.height);
