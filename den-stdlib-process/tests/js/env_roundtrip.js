import { assertEquals } from "den:assert";
const key = `DEN_PROCESS_TEST_${process.pid}`;
process.env[key] = 123;
assertEquals(process.env[key], "123");
assertEquals(key in process.env, true);
delete process.env[key];
assertEquals(process.env[key], undefined);
