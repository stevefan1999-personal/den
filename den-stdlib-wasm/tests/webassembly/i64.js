import { assert, assertEquals, assertThrows } from "den:assert";
import { wat2wasm } from "den:wasm";

const WASM = wat2wasm(`
    (module
      (func (export "echo") (param i64) (result i64) local.get 0)
      (func (export "beyond") (result i64) i64.const 9007199254740993))
`);
const { echo, beyond } = (await WebAssembly.instantiate(WASM)).instance.exports;
assertEquals(typeof beyond(), "bigint");
assertEquals(beyond(), 9007199254740993n);
assertEquals(echo(-9007199254740993n), -9007199254740993n);
assertThrows(() => echo(1), TypeError);
