import { x as top } from "pkg";
import { x as inner } from "./nested/mod.js";

globalThis.got = `${top},${inner}`;
