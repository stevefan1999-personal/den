const assert = (holds, what) => { if (!holds) throw new Error(what); };

const memory = new WebAssembly.Memory({ initial: 1, maximum: 2 });
const buffer = memory.buffer;
assert(buffer === memory.buffer, "buffer is not the same object between grows");
assert(buffer.byteLength === 65536, "buffer does not span the linear memory");
new Uint8Array(buffer)[0] = 7;
assert(memory.grow(1) === 1, "grow does not return the previous page count");
assert(buffer.byteLength === 0, "the old buffer was not detached");
assert(memory.buffer.byteLength === 131072, "the fresh buffer has the wrong size");
assert(new Uint8Array(memory.buffer)[0] === 7, "growing lost the memory contents");

const table = new WebAssembly.Table({ element: "anyfunc", initial: 2 });
assert(table.length === 2, "length is wrong");
assert(table.get(0) === null, "a fresh anyfunc table is not full of nulls");
table.set(1, null);
assert(table.grow(1) === 2, "grow does not return the previous length");
assert(table.length === 3, "grow did not grow");

const mutable = new WebAssembly.Global({ value: "i32", mutable: true }, 5);
assert(mutable.value === 5, "the value getter is missing or wrong");
mutable.value = 6;
assert(mutable.valueOf() === 6, "the value setter or valueOf is missing or wrong");

const constant = new WebAssembly.Global({ value: "i64" }, 1n);
assert(constant.value === 1n, "an i64 global does not read as a BigInt");
try {
  constant.value = 2n;
  throw new Error("an immutable global accepted a write");
} catch (error) {
  assert(error instanceof TypeError, "writing an immutable global is not a TypeError");
}
true
