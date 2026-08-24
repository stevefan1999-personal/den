import { assertEquals } from "den:assert";
const digest = await crypto.subtle.digest("SHA-256", new TextEncoder().encode("abc"));
const hex = [...new Uint8Array(digest)]
  .map((byte) => byte.toString(16).padStart(2, "0"))
  .join("");
assertEquals(hex, "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad");
