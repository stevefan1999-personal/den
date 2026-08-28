import { assertEquals } from "den:assert";
import { WASM } from "./hello.js";

const module = await WebAssembly.compile(WASM);
const section = WebAssembly.Module.customSections(module, "hello")[0];
const assembled = globalThis.wat2wasm("(module)");
const grown = assembled.buffer.transfer(assembled.byteLength + 8);
const shrunk = grown.transfer(4);
globalThis.wat2wasm("(module)").buffer.transfer(0);
const moved = section.transfer();
const fixture = WASM.buffer.transfer();
assertEquals(
  [
    new Uint8Array(shrunk).join("-"),
    String.fromCharCode(...new Uint8Array(moved)),
    assembled.buffer.detached,
    grown.detached,
    section.detached,
    fixture.byteLength > 0,
    WASM.byteLength,
  ].join(","),
  "0-97-115-109,world,true,true,true,true,0",
);
