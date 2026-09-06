// cargo run -- examples/crypto.ts
//
// Web Crypto as globals: SubtleCrypto.digest (SHA-1/256/384/512),
// crypto.getRandomValues, and crypto.randomUUID.

import { assert, assertEquals } from "den:assert";

function hex(bytes: ArrayBuffer): string {
  return [...new Uint8Array(bytes)]
    .map((byte) => byte.toString(16).padStart(2, "0"))
    .join("");
}

const digest = await crypto.subtle.digest("SHA-256", new TextEncoder().encode("abc"));
const entropy = crypto.getRandomValues(new Uint8Array(8));
const uuid = crypto.randomUUID();
assertEquals(
  hex(digest),
  "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad",
);
assert(/^[0-9a-f]{8}-[0-9a-f]{4}-4[0-9a-f]{3}-[89ab][0-9a-f]{3}-[0-9a-f]{12}$/.test(uuid));
assert(entropy.some((byte) => byte !== 0));
console.log("sha-256(abc)", hex(digest));
console.log("uuid", uuid);
console.log("random", hex(entropy.buffer));
