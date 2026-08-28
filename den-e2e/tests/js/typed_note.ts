import { assertEquals } from "den:assert";

function add(left: number, right: number): number {
  return left + right;
}

const total: number = add(40, 2);
assertEquals(total, 42);
