import { assert, assertEquals, assertThrows } from "den:assert";

const PAGE = 65536;
const memory = new WebAssembly.Memory({ initial: 1, maximum: 4 });
const fixed = memory.buffer;
assertEquals(fixed.resizable, false);
// A fixed-length buffer is already what toFixedLengthBuffer promises, so the
// same object comes back and nothing is detached.
assert(memory.toFixedLengthBuffer() === fixed);
assert(memory.buffer === fixed);
assertEquals(fixed.byteLength, PAGE);

const resizable = memory.toResizableBuffer();
assertEquals(resizable.resizable, true);
assertEquals(resizable.byteLength, PAGE);
assertEquals(resizable.maxByteLength, 4 * PAGE);
assertEquals(fixed.byteLength, 0, "switching kinds detaches the previous buffer");
assert(memory.toResizableBuffer() === resizable);
assert(memory.buffer === resizable);

const fixedAgain = memory.toFixedLengthBuffer();
assert(fixedAgain !== fixed);
assertEquals(fixedAgain.resizable, false);
assertEquals(resizable.byteLength, 0);
assert(memory.toFixedLengthBuffer() === fixedAgain);
assert(memory.buffer === fixedAgain);

memory.grow(1);
assertEquals(fixedAgain.byteLength, 0);
assertEquals(memory.buffer.byteLength, 2 * PAGE);

assertThrows(
  () => new WebAssembly.Memory({ initial: 1 }).toResizableBuffer(),
  TypeError,
  "maximum",
);
