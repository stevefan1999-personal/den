import {
  assertEquals,
  assertNotEquals,
  assertObjectMatch,
  assertThrows,
  AssertionError,
} from "den:assert";

const left = { name: "cycle" };
const right = { name: "cycle" };
left.self = left;
right.self = right;
assertEquals(left, right);

assertEquals(new Date(1234), new Date(1234));
assertNotEquals(new Date(1234), new Date(5678));
assertNotEquals(new Date(1234), {});
assertEquals(/den/gi, /den/gi);
assertNotEquals(/den/g, /den/i);
assertNotEquals(/den/g, {});
assertObjectMatch(
  { nested: { answer: 42, ignored: true }, list: [1, 2] },
  { nested: { answer: 42 } },
);

assertThrows(
  () => assertEquals({ answer: 41 }, { answer: 42 }, "custom equality message"),
  AssertionError,
  "custom equality message",
);
