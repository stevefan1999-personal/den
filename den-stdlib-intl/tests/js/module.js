import { assertEquals, assertStrictEquals } from "den:assert";
import { Intl as imported } from "den:intl";

// The module and the global have to name the same object, or `instanceof`
// disagrees with itself across the two.
assertStrictEquals(imported, globalThis.Intl);
assertStrictEquals(new imported.Locale("en") instanceof Intl.Locale, true);

const descriptor = Object.getOwnPropertyDescriptor(globalThis, "Intl");
assertEquals(
  { writable: descriptor.writable, enumerable: descriptor.enumerable, configurable: descriptor.configurable },
  { writable: true, enumerable: false, configurable: true },
);
assertStrictEquals(Object.prototype.toString.call(Intl), "[object Intl]");
