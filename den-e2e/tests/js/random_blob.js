import { assert, assertEquals } from "den:assert";

const bytes = new Uint8Array(16);
crypto.getRandomValues(bytes);
assert(bytes.some((byte) => byte !== 0));
assertEquals(crypto.randomUUID().length, 36);

const blob = new Blob([bytes], { type: "application/octet-stream" });
assertEquals(blob.size, 16);
const reader = new FileReader();
const buffer = await new Promise((resolve, reject) => {
  reader.onload = () => resolve(reader.result);
  reader.onerror = () => reject(reader.error);
  reader.readAsArrayBuffer(blob);
});
assertEquals(new Uint8Array(buffer).length, 16);
assertEquals(atob(btoa("den")), "den");
