import { assertStrictEquals } from "den:assert";
import { createImageBitmap, ImageBitmap, ImageData } from "den:canvas";

// The module and the globals are the same objects, so a class installed by one
// is recognised by the other.
assertStrictEquals(ImageData, globalThis.ImageData);
assertStrictEquals(ImageBitmap, globalThis.ImageBitmap);
assertStrictEquals(createImageBitmap, globalThis.createImageBitmap);

const bitmap = await createImageBitmap(new ImageData(1, 1));
assertStrictEquals(bitmap instanceof ImageBitmap, true);
