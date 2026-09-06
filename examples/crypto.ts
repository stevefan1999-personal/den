// cargo run -- examples/crypto.ts
//
// Web Crypto as globals: SubtleCrypto.digest (SHA-1/256/384/512),
// crypto.getRandomValues, and crypto.randomUUID.

export {};

function hex(bytes: ArrayBuffer): string {
  return [...new Uint8Array(bytes)]
    .map((byte) => byte.toString(16).padStart(2, "0"))
    .join("");
}

const digest = await crypto.subtle.digest("SHA-256", new TextEncoder().encode("abc"));
const entropy = crypto.getRandomValues(new Uint8Array(8));
console.log("sha-256(abc)", hex(digest));
console.log("uuid", crypto.randomUUID());
console.log("random", hex(entropy.buffer));
