import { assertEquals, assertRejects } from "den:assert";

const bitmapData = Symbol.for("den.bitmapData");
const read = (bitmap) => Array.from(bitmap[bitmapData]());

const red = [255, 0, 0, 255];
const green = [0, 255, 0, 255];
const blue = [0, 0, 255, 255];
const amber = [255, 255, 0, 128];

const source = new ImageData(2, 2);
source.data.set([...red, ...green, ...blue, ...amber]);

// resizeWidth / resizeHeight, with nearest-neighbour so the pixels are exact.
const doubled = await createImageBitmap(source, {
  resizeWidth: 4,
  resizeHeight: 4,
  resizeQuality: "pixelated",
});
assertEquals(doubled.width, 4);
assertEquals(doubled.height, 4);
assertEquals(read(doubled).slice(0, 16), [...red, ...red, ...green, ...green]);

// One dimension given fixes the other by aspect ratio.
assertEquals((await createImageBitmap(source, { resizeWidth: 6 })).height, 6);
assertEquals((await createImageBitmap(source, { resizeHeight: 3 })).width, 3);

// A smooth downscale averages; "low", "medium" and "high" agree.
const halved = await createImageBitmap(source, { resizeWidth: 1, resizeHeight: 1 });
assertEquals(read(halved), [128, 128, 64, 223]);
for (const resizeQuality of ["low", "medium", "high"]) {
  const other = await createImageBitmap(source, { resizeWidth: 1, resizeHeight: 1, resizeQuality });
  assertEquals(read(other), read(halved));
}
// "pixelated" takes the nearest sample instead.
assertEquals(
  read(await createImageBitmap(source, {
    resizeWidth: 1,
    resizeHeight: 1,
    resizeQuality: "pixelated",
  })),
  amber,
);

// An empty resize target is an InvalidStateError, not a silent no-op, and it
// too is reported before the source type is examined.
await assertRejects(() => createImageBitmap(source, { resizeWidth: 0 }), DOMException);
await assertRejects(() => createImageBitmap(source, { resizeHeight: 0 }), DOMException);
await assertRejects(() => createImageBitmap(null, { resizeWidth: 0 }), DOMException);

// imageOrientation. "none" is the HTML spec's current spelling of
// "from-image"; den accepts both where Deno rejects "none".
const flipped = await createImageBitmap(source, { imageOrientation: "flipY" });
assertEquals(read(flipped), [...blue, ...amber, ...red, ...green]);
for (const imageOrientation of ["from-image", "none", undefined]) {
  assertEquals(read(await createImageBitmap(source, { imageOrientation })), Array.from(source.data));
}
await assertRejects(() => createImageBitmap(source, { imageOrientation: "flipX" }), TypeError);

// premultiplyAlpha. ImageData is straight, so "default" and "none" leave it be
// and "premultiply" scales the colour channels by alpha.
for (const premultiplyAlpha of ["default", "none", undefined]) {
  assertEquals(read(await createImageBitmap(source, { premultiplyAlpha })), Array.from(source.data));
}
const premultiplied = await createImageBitmap(source, { premultiplyAlpha: "premultiply" });
assertEquals(read(premultiplied), [...red, ...green, ...blue, 128, 128, 0, 128]);
// den tracks the alpha state rather than guessing it from the samples, so the
// inverse request on the resulting bitmap round-trips.
assertEquals(
  read(await createImageBitmap(premultiplied, { premultiplyAlpha: "none" })),
  Array.from(source.data),
);
await assertRejects(() => createImageBitmap(source, { premultiplyAlpha: "yes" }), TypeError);

// colorSpaceConversion is validated but has nothing to act on: without a codec
// there is no embedded profile, so both members hand the samples over as they
// are, and an unlisted member is still a TypeError.
const wide = new ImageData(1, 1, { colorSpace: "display-p3" });
wide.data.set([128, 64, 32, 255]);
for (const colorSpaceConversion of ["default", "none", undefined]) {
  assertEquals(read(await createImageBitmap(wide, { colorSpaceConversion })), Array.from(wide.data));
}
await assertRejects(() => createImageBitmap(source, { colorSpaceConversion: "srgb" }), TypeError);

// A non-object options argument is not a dictionary.
await assertRejects(() => createImageBitmap(source, "options"), TypeError);
