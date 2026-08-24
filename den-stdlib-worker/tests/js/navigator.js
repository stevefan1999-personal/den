import { assert, assertEquals } from "den:assert";

const uad = navigator.userAgentData;
assert(uad);
assert(uad instanceof NavigatorUAData);
assertEquals(Object.prototype.toString.call(uad), "[object NavigatorUAData]");
assert(Array.isArray(uad.brands) && uad.brands.length > 0);
const brand = uad.brands[0];
assertEquals(typeof brand.brand, "string");
assertEquals(typeof brand.version, "string");
assertEquals(brand.brand, "den");
assert(Object.isFrozen(uad.brands));
assert(Object.isFrozen(brand));
assertEquals(uad.mobile, false);
assert(
  ["Linux", "macOS", "Windows", "FreeBSD", "OpenBSD"].includes(uad.platform)
    || (typeof uad.platform === "string" && uad.platform.length > 0),
);
const json = uad.toJSON();
assert(Array.isArray(json.brands));
assertEquals(json.mobile, uad.mobile);
assertEquals(json.platform, uad.platform);
assert(/^den\/\d+\.\d+\.\d+/.test(navigator.userAgent));
const major = navigator.userAgent.slice("den/".length).split(".")[0];
assertEquals(brand.version, major);
assert(Number.isInteger(navigator.hardwareConcurrency) && navigator.hardwareConcurrency >= 1);
const descriptor = Object.getOwnPropertyDescriptor(globalThis, "navigator");
assertEquals(descriptor.enumerable, true);
assertEquals(descriptor.writable, false);
assertEquals(descriptor.configurable, false);
const before = navigator;
try {
  navigator = {};
} catch {
  /* strict assignment throws */
}
assertEquals(navigator, before);
assertEquals(Object.prototype.toString.call(navigator), "[object Navigator]");

const highEntropy = uad.getHighEntropyValues([
  "architecture", "bitness", "fullVersionList", "model",
  "platformVersion", "wow64", "formFactors",
]);
const empty = uad.getHighEntropyValues([]);
let invalidHintsTypeError = false;
const invalid = uad.getHighEntropyValues("not an array").then(
  () => {
    throw new Error("invalidHintsShouldReject");
  },
  (error) => {
    invalidHintsTypeError = error instanceof TypeError;
  },
);
const [hev, none] = await Promise.all([highEntropy, empty, invalid]);
assert(invalidHintsTypeError);
assert(Array.isArray(hev.brands));
assertEquals(typeof hev.mobile, "boolean");
assertEquals(typeof hev.platform, "string");
assertEquals(typeof hev.architecture, "string");
assertEquals(typeof hev.bitness, "string");
assert(Array.isArray(hev.fullVersionList));
assertEquals(hev.fullVersionList[0].brand, "den");
assert(hev.fullVersionList[0].version.includes("."));
assertEquals(typeof hev.model, "string");
assertEquals(typeof hev.platformVersion, "string");
assert(hev.platformVersion.split(".").length >= 3);
assertEquals(typeof hev.wow64, "boolean");
assert(Array.isArray(hev.formFactors));
assert(Array.isArray(none.brands));
assertEquals(none.architecture, undefined);
