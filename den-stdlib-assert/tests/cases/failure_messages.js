import {
  assert,
  assertEquals,
  assertFalse,
  assertRejects,
  assertSnapshot,
  assertStrictEquals,
  assertStringIncludes,
  assertThrows,
} from "den:assert";

const messages = [];
const capture = (name, callback) => {
  try {
    callback();
    throw new Error(`${name} did not fail`);
  } catch (error) {
    messages.push(`${name}: ${error.message}`);
  }
};

capture("assert", () => assert(false));
capture("assertFalse", () => assertFalse(true));
capture("assertEquals", () => assertEquals(1, 2));
capture("assertStrictEquals", () => assertStrictEquals(1, "1"));
capture("assertStringIncludes", () => assertStringIncludes("hello", "xyz"));
capture("assertThrows", () => assertThrows(() => {}));

try {
  await assertRejects(() => Promise.resolve("ok"));
  throw new Error("assertRejects did not fail");
} catch (error) {
  messages.push(`assertRejects: ${error.message}`);
}

assertSnapshot(messages.join("\n"), "den_assert_failure_messages");
