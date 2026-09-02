import { assertThrows } from "den:assert";

// Arity: WebIDL requires two arguments before any overload can match.
assertThrows(() => new ImageData(1), TypeError);
assertThrows(() => new ImageData(), TypeError);

// Data length faults are TypeErrors.
assertThrows(() => new ImageData(new Uint8ClampedArray(0), 1), TypeError);
assertThrows(() => new ImageData(new Uint8ClampedArray(6), 1), TypeError);

// Dimension faults are RangeErrors.
assertThrows(() => new ImageData(0, 1), RangeError);
assertThrows(() => new ImageData(1, 0), RangeError);
assertThrows(() => new ImageData(new Uint8ClampedArray(8), 0), RangeError);
assertThrows(() => new ImageData(new Uint8ClampedArray(8), 1, 0), RangeError);
// 8 bytes is two pixels: not a whole number of rows three wide.
assertThrows(() => new ImageData(new Uint8ClampedArray(8), 3), RangeError);
// Two pixels, one wide, is two rows and not the three claimed.
assertThrows(() => new ImageData(new Uint8ClampedArray(8), 1, 3), RangeError);

// An unknown enumeration member is a TypeError.
assertThrows(() => new ImageData(1, 1, { colorSpace: "rec2020" }), TypeError);
assertThrows(() => new ImageData(1, 1, { colorSpace: "SRGB" }), TypeError);
// A non-object settings argument is not a dictionary.
assertThrows(() => new ImageData(1, 1, 5), TypeError);

// Real overload resolution: only a Uint8ClampedArray picks the data overload,
// so a Uint8Array falls through to `unsigned long`, coerces to 0 and is a
// RangeError rather than being silently accepted as pixel data.
assertThrows(() => new ImageData(new Uint8Array(16), 4), RangeError);
assertThrows(() => new ImageData(new Float64Array(16), 4), RangeError);

// The constructor is not callable without `new`.
assertThrows(() => ImageData(1, 1), TypeError);
