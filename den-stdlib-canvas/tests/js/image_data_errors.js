import { assertEquals, assertThrows } from "den:assert";

const assertThrowsDom = (fn, name) => {
  const thrown = assertThrows(fn, DOMException);
  assertEquals(thrown.name, name);
};

// Arity: WebIDL requires two arguments before any overload can match.
assertThrows(() => new ImageData(1), TypeError);
assertThrows(() => new ImageData(), TypeError);

// Data length faults are "InvalidStateError" DOMExceptions.
assertThrowsDom(() => new ImageData(new Uint8ClampedArray(0), 1), "InvalidStateError");
assertThrowsDom(() => new ImageData(new Uint8ClampedArray(6), 1), "InvalidStateError");

// Dimension faults are "IndexSizeError" DOMExceptions.
assertThrowsDom(() => new ImageData(0, 1), "IndexSizeError");
assertThrowsDom(() => new ImageData(1, 0), "IndexSizeError");
assertThrowsDom(() => new ImageData(new Uint8ClampedArray(8), 0), "IndexSizeError");
assertThrowsDom(() => new ImageData(new Uint8ClampedArray(8), 1, 0), "IndexSizeError");
// 8 bytes is two pixels: not a whole number of rows three wide.
assertThrowsDom(() => new ImageData(new Uint8ClampedArray(8), 3), "IndexSizeError");
// Two pixels, one wide, is two rows and not the three claimed.
assertThrowsDom(() => new ImageData(new Uint8ClampedArray(8), 1, 3), "IndexSizeError");

// An unknown enumeration member is a TypeError.
assertThrows(() => new ImageData(1, 1, { colorSpace: "rec2020" }), TypeError);
assertThrows(() => new ImageData(1, 1, { colorSpace: "SRGB" }), TypeError);
// A non-object settings argument is not a dictionary.
assertThrows(() => new ImageData(1, 1, 5), TypeError);

// Real overload resolution: only a Uint8ClampedArray picks the data overload,
// so a Uint8Array falls through to `unsigned long`, coerces to 0 and is a
// zero-height fault rather than being silently accepted as pixel data.
assertThrowsDom(() => new ImageData(new Uint8Array(16), 4), "IndexSizeError");
assertThrowsDom(() => new ImageData(new Float64Array(16), 4), "IndexSizeError");

// The constructor is not callable without `new`.
assertThrows(() => ImageData(1, 1), TypeError);
