import { assertEquals } from "den:assert";
const id = setTimeout(() => {}, 0);
assertEquals(typeof clearTimeout, "function");
assertEquals(typeof id, "number");
