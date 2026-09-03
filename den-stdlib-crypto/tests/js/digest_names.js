import { assertEquals } from "den:assert";

// AlgorithmIdentifier normalisation: names match ASCII-case-insensitively,
// a dictionary contributes its `name`, and anything else is NotSupportedError
// naming what was read ("undefined" when nothing string-shaped was).
const hex = (buffer) =>
  [...new Uint8Array(buffer)].map((byte) => byte.toString(16).padStart(2, "0")).join("");
const abc = new TextEncoder().encode("abc");
const sha1 = "a9993e364706816aba3e25717850c26c9cd0d89d";
assertEquals(hex(await crypto.subtle.digest("sha-1", abc)), sha1);
assertEquals(hex(await crypto.subtle.digest({ name: "Sha-1" }, abc)), sha1);

const failure = async (algorithm) => {
  try {
    await crypto.subtle.digest(algorithm, abc);
  } catch (error) {
    return `${error.name}: ${error.message}`;
  }
  return "no rejection";
};
assertEquals(await failure("SHA-0"), "NotSupportedError: Unrecognized algorithm name: SHA-0");
assertEquals(await failure({ name: "md5" }), "NotSupportedError: Unrecognized algorithm name: md5");
assertEquals(await failure({}), "NotSupportedError: Unrecognized algorithm name: undefined");
assertEquals(await failure({ name: 5 }), "NotSupportedError: Unrecognized algorithm name: undefined");
assertEquals(await failure({ name: null }), "NotSupportedError: Unrecognized algorithm name: undefined");
assertEquals(await failure(7), "NotSupportedError: Unrecognized algorithm name: undefined");
