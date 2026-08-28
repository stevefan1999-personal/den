const before = new Uint8Array(chunk).join("-");
const moved = chunk.buffer.transfer(2);
[before, new Uint8Array(moved).join("-"),
 chunk.buffer.detached, chunk.byteLength].join(",")
