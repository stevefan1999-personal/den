import { assertEquals, assertRejects } from "den:assert";

const bitmapData = Symbol.for("den.bitmapData");
const read = (bitmap) => Array.from(bitmap[bitmapData]());

const red = [255, 0, 0, 255];
const green = [0, 255, 0, 255];
const blue = [0, 0, 255, 255];
const amber = [255, 255, 0, 128];
const clear = [0, 0, 0, 0];

const source = new ImageData(2, 2);
source.data.set([...red, ...green, ...blue, ...amber]);

// A crop wholly inside the source is a plain copy of that rectangle.
const inside = await createImageBitmap(source, 1, 0, 1, 2);
assertEquals(inside.width, 1);
assertEquals(inside.height, 2);
assertEquals(read(inside), [...green, ...amber]);

// The spec crops against an infinite transparent-black surface, so anything
// past the edge pads rather than clamps.
const overhang = await createImageBitmap(source, 1, 1, 2, 2);
assertEquals(read(overhang), [...amber, ...clear, ...clear, ...clear]);

const before = await createImageBitmap(source, -1, -1, 2, 2);
assertEquals(read(before), [...clear, ...clear, ...clear, ...red]);

// Entirely outside: every pixel is transparent black.
const away = await createImageBitmap(source, 10, 10, 2, 2);
assertEquals(read(away), [...clear, ...clear, ...clear, ...clear]);

// A negative extent grows the rectangle the other way.
const backwards = await createImageBitmap(source, 2, 2, -2, -2);
assertEquals(read(backwards), Array.from(source.data));

// An empty source rectangle is a RangeError.
await assertRejects(() => createImageBitmap(source, 0, 0, 0, 2), RangeError);
await assertRejects(() => createImageBitmap(source, 0, 0, 2, 0), RangeError);
