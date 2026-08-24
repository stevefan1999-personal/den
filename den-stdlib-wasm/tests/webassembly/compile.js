import { assertEquals } from "den:assert";

let name = "no rejection";
try {
  await WebAssembly.compile(new Uint8Array([0, 97, 115, 109, 9, 9, 9, 9]));
} catch (error) {
  name = error.name;
}
assertEquals(name, "CompileError");
