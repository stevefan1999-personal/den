import { assertEquals } from "den:assert";

const hex = (buffer) =>
  [...new Uint8Array(buffer)].map((byte) => byte.toString(16).padStart(2, "0")).join("");
const expected = "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad";
const abc = new TextEncoder().encode("abc");
const padded = new ArrayBuffer(5);
new Uint8Array(padded).set([0, 97, 98, 99, 0]);
const view = new DataView(padded, 1, 3);
assertEquals(hex(await crypto.subtle.digest("sha-256", abc)), expected);
assertEquals(hex(await crypto.subtle.digest({ name: "SHA-256" }, abc)), expected);
assertEquals(hex(await crypto.subtle.digest("SHA-256", view)), expected);
assertEquals(hex(await crypto.subtle.digest("SHA-256", abc.buffer)), expected);
