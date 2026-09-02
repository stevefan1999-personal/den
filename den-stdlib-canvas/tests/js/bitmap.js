import { assertEquals, assertRejects, assertStrictEquals, assertThrows } from "den:assert";

// ImageBitmap keeps no spec-visible pixels; this is den's read-back hook.
const bitmapData = Symbol.for("den.bitmapData");
const read = (bitmap) => Array.from(bitmap[bitmapData]());

const source = new ImageData(2, 2);
source.data.set([
  255, 0, 0, 255, /**/ 0, 255, 0, 255,
  0, 0, 255, 255, /**/ 255, 255, 0, 128,
]);

const pending = createImageBitmap(source);
assertStrictEquals(pending.constructor, Promise);

const bitmap = await pending;
assertEquals(bitmap.width, 2);
assertEquals(bitmap.height, 2);
assertEquals(Object.prototype.toString.call(bitmap), "[object ImageBitmap]");
assertEquals(read(bitmap), Array.from(source.data));

// An ImageBitmap is itself a source, and copying it is lossless.
const copy = await createImageBitmap(bitmap);
assertEquals(read(copy), Array.from(source.data));

// close() detaches: the spec reports zero for both dimensions afterwards.
copy.close();
assertEquals(copy.width, 0);
assertEquals(copy.height, 0);
assertEquals(read(copy), []);
// close() is idempotent.
copy.close();
assertEquals(copy.width, 0);
// A detached bitmap is no longer a usable source.
await assertRejects(() => createImageBitmap(copy), DOMException);

// The pixels read back are the view's window, not the whole buffer.
const shared = new Uint8ClampedArray(3 * 4);
shared.set([9, 8, 7, 6], 4);
const windowed = await createImageBitmap(
  new ImageData(new Uint8ClampedArray(shared.buffer, 4, 4), 1),
);
assertEquals(read(windowed), [9, 8, 7, 6]);

// An ImageData whose buffer has been transferred away has no pixels to read.
const transferred = new ImageData(1, 1);
transferred.data.buffer.transfer();
await assertRejects(() => createImageBitmap(transferred), DOMException);

// The interface is exposed but not constructible.
assertThrows(() => new ImageBitmap(), TypeError);
assertEquals(typeof ImageBitmap, "function");

// Sources phase 0 cannot decode are rejected, not ignored.
await assertRejects(() => createImageBitmap(new Uint8ClampedArray(4)), TypeError);
await assertRejects(() => createImageBitmap(null), TypeError);
await assertRejects(() => createImageBitmap(), TypeError);
// Neither overload takes three or four arguments.
await assertRejects(() => createImageBitmap(source, 0, 0), TypeError);
await assertRejects(() => createImageBitmap(source, 0, 0, 1), TypeError);

assertEquals(createImageBitmap.name, "createImageBitmap");
assertEquals(createImageBitmap.length, 1);
