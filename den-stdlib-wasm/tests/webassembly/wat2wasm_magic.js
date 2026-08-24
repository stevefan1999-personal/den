import { assertEquals } from "den:assert";
import { WASM } from "./add.js";

assertEquals([...new Uint8Array(WASM.slice(0, 4))].join(","), "0,97,115,109");
