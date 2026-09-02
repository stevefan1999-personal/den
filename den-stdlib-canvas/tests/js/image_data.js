import { assertEquals, assertStrictEquals } from "den:assert";

// new ImageData(sw, sh) allocates transparent black.
const blank = new ImageData(3, 2);
assertEquals(blank.width, 3);
assertEquals(blank.height, 2);
assertEquals(blank.colorSpace, "srgb");
assertEquals(blank.data.constructor, Uint8ClampedArray);
assertEquals(blank.data.length, 3 * 2 * 4);
assertEquals(Array.from(blank.data).every((byte) => byte === 0), true);
assertEquals(Object.prototype.toString.call(blank), "[object ImageData]");

// `data` is one live object, not a copy per read: writes must survive.
assertStrictEquals(blank.data, blank.data);
blank.data[0] = 200;
assertEquals(blank.data[0], 200);

// new ImageData(data, sw) derives the height.
const pixels = new Uint8ClampedArray(2 * 3 * 4);
const derived = new ImageData(pixels, 2);
assertEquals(derived.width, 2);
assertEquals(derived.height, 3);
assertStrictEquals(derived.data, pixels);

// new ImageData(data, sw, sh) accepts a matching height.
assertEquals(new ImageData(pixels, 3, 2).height, 2);

// A view onto part of a larger buffer keeps its own window.
const shared = new ArrayBuffer(3 * 4);
const window = new Uint8ClampedArray(shared, 4, 4);
window.set([9, 8, 7, 6]);
const windowed = new ImageData(window, 1);
assertEquals(windowed.width, 1);
assertEquals(windowed.height, 1);
assertStrictEquals(windowed.data, window);
assertEquals(windowed.data.byteOffset, 4);

// Settings ride along on either overload.
assertEquals(new ImageData(1, 1, { colorSpace: "display-p3" }).colorSpace, "display-p3");
assertEquals(
  new ImageData(pixels, 2, 3, { colorSpace: "display-p3" }).colorSpace,
  "display-p3",
);
assertEquals(new ImageData(1, 1, { colorSpace: undefined }).colorSpace, "srgb");
assertEquals(new ImageData(1, 1, undefined).colorSpace, "srgb");
assertEquals(new ImageData(1, 1, null).colorSpace, "srgb");

// The clamping is the array's, not ours.
const clamped = new ImageData(1, 1);
clamped.data[0] = 300;
clamped.data[1] = -20;
assertEquals(clamped.data[0], 255);
assertEquals(clamped.data[1], 0);

// Accessors live on the prototype and are enumerable, per WebIDL.
const descriptor = Object.getOwnPropertyDescriptor(ImageData.prototype, "width");
assertEquals(typeof descriptor.get, "function");
assertEquals(descriptor.enumerable, true);
