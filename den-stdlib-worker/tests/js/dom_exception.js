import { assertEquals } from "den:assert";

const error = new DOMException("why", "DataCloneError");
assertEquals(typeof DOMException, "function");
assertEquals(error instanceof Error, true);
assertEquals(error.name, "DataCloneError");
assertEquals(error.message, "why");
