import { assertEquals } from "den:assert";
import * as denWasm from "den:wasm";

assertEquals("wasiImports" in denWasm, false);
